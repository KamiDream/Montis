//! 播放引擎：下载好的音频字节 → rodio 解码输出，同时把采样喂给频谱分析。
//!
//! rodio 的 `OutputStream`（底层 cpal）不能跨线程移动，所以整套音频对象都活在
//! 一个专用线程里，主线程只通过命令通道和几个原子量交互。

use std::io::Cursor;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rodio::{Decoder, OutputStream, Sink, Source};

/// 环形缓冲的采样数。8192 个交错采样 @44.1kHz 立体声 ≈ 93ms，够 2048 点 FFT。
pub const RING_SIZE: usize = 8_192;

/// 音频线程每次锁环形缓冲最多写入多少采样。
/// 按批写入可以把每秒 8 万多次锁降到约 170 次，避免抢占音频回调。
const FLUSH_SAMPLES: usize = 512;
/// 每累计多少帧刷新一次播放位置（≈ 12ms @44.1kHz）。
const POSITION_FRAMES: u64 = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum State {
    /// 还没有加载任何音频。
    #[default]
    Idle,
    Playing,
    Paused,
    /// 当前曲目播放完毕。
    Finished,
    /// 没有可用的音频设备，或音频无法解码。
    Failed,
}

#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    pub state: State,
    pub position: Duration,
    pub duration: Duration,
}

#[derive(Debug)]
enum Command {
    Load(Vec<u8>),
    Play,
    Pause,
    Toggle,
    Stop,
    Seek(Duration),
    Volume(f32),
    Shutdown,
}

/// 主线程持有的播放器句柄。
pub struct Engine {
    tx: Sender<Command>,
    shared: Arc<Shared>,
}

impl Engine {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Shared::default());

        let thread_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("melo-audio".to_string())
            .spawn(move || audio_thread(rx, thread_shared))
            .expect("无法启动音频线程");

        Self { tx, shared }
    }

    pub fn load(&self, bytes: Vec<u8>) {
        let _ = self.tx.send(Command::Load(bytes));
    }

    pub fn play(&self) {
        let _ = self.tx.send(Command::Play);
    }

    pub fn pause(&self) {
        let _ = self.tx.send(Command::Pause);
    }

    pub fn toggle(&self) {
        let _ = self.tx.send(Command::Toggle);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }

    pub fn seek(&self, position: Duration) {
        let _ = self.tx.send(Command::Seek(position));
    }

    pub fn set_volume(&self, volume: f32) {
        let _ = self.tx.send(Command::Volume(volume.clamp(0.0, 1.0)));
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            state: self.shared.state(),
            position: Duration::from_millis(self.shared.position_ms.load(Ordering::Relaxed)),
            duration: Duration::from_millis(self.shared.duration_ms.load(Ordering::Relaxed)),
        }
    }

    /// 最近一段交错采样、声道数与采样率。不足一帧时采样为空。
    #[allow(clippy::type_complexity)]
    pub fn samples(&self) -> (Vec<f32>, usize, u32) {
        let mut buffer = Vec::with_capacity(RING_SIZE);
        let (channels, sample_rate) = self.samples_into(&mut buffer);
        (buffer, channels, sample_rate)
    }

    /// 同 [`Self::samples`]，但复用调用方的缓冲区，避免每帧分配。
    pub fn samples_into(&self, buffer: &mut Vec<f32>) -> (usize, u32) {
        let channels = self.shared.channels.load(Ordering::Relaxed) as usize;
        let sample_rate = self.shared.sample_rate.load(Ordering::Relaxed);
        self.shared.ring.lock().unwrap().snapshot_into(buffer);
        (
            channels.max(1),
            if sample_rate == 0 {
                44_100
            } else {
                sample_rate
            },
        )
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Shutdown);
    }
}

#[derive(Default)]
struct Shared {
    ring: Mutex<Ring>,
    state: Mutex<State>,
    error: Mutex<Option<String>>,
    position_ms: AtomicU64,
    duration_ms: AtomicU64,
    channels: AtomicU32,
    sample_rate: AtomicU32,
}

impl Shared {
    fn state(&self) -> State {
        *self.state.lock().unwrap()
    }

    fn set_state(&self, state: State) {
        *self.state.lock().unwrap() = state;
    }

    fn fail(&self, message: impl Into<String>) {
        *self.error.lock().unwrap() = Some(message.into());
        self.set_state(State::Failed);
    }

    fn reset_spectrum(&self) {
        self.reset_samples();
        self.duration_ms.store(0, Ordering::Relaxed);
    }

    /// 只清空当前采样与进度；换歌 / seek 后调用，保留已经解析到的时长。
    fn reset_samples(&self) {
        self.ring.lock().unwrap().reset();
        self.position_ms.store(0, Ordering::Relaxed);
    }

    fn push_samples(&self, samples: &[f32]) {
        self.ring.lock().unwrap().push_batch(samples);
    }
}

struct Ring {
    buffer: Vec<f32>,
    write: usize,
    filled: usize,
}

impl Default for Ring {
    fn default() -> Self {
        Self {
            buffer: vec![0.0; RING_SIZE],
            write: 0,
            filled: 0,
        }
    }
}

impl Ring {
    fn reset(&mut self) {
        self.write = 0;
        self.filled = 0;
        self.buffer.fill(0.0);
    }

    #[cfg(test)]
    fn push(&mut self, sample: f32) {
        self.buffer[self.write] = sample;
        self.write = (self.write + 1) % self.buffer.len();
        self.filled = (self.filled + 1).min(self.buffer.len());
    }

    fn push_batch(&mut self, samples: &[f32]) {
        let len = self.buffer.len();
        if samples.len() >= len {
            // 只有最新的一圈有意义。
            let tail = &samples[samples.len() - len..];
            self.buffer.copy_from_slice(tail);
            self.write = 0;
            self.filled = len;
            return;
        }
        for sample in samples {
            self.buffer[self.write] = *sample;
            self.write = (self.write + 1) % len;
        }
        self.filled = (self.filled + samples.len()).min(len);
    }

    /// 按时间顺序返回已填充的采样。
    #[cfg(test)]
    fn snapshot(&self) -> Vec<f32> {
        let mut out = Vec::with_capacity(self.filled);
        self.snapshot_into(&mut out);
        out
    }

    fn snapshot_into(&self, out: &mut Vec<f32>) {
        out.clear();
        out.reserve(self.filled);
        let len = self.buffer.len();
        let start = if self.filled == len { self.write } else { 0 };
        for offset in 0..self.filled {
            out.push(self.buffer[(start + offset) % len]);
        }
    }
}

fn audio_thread(rx: mpsc::Receiver<Command>, shared: Arc<Shared>) {
    let (stream, handle) = match OutputStream::try_default() {
        Ok(ok) => ok,
        Err(error) => {
            shared.fail(format!("没有可用的音频输出设备：{error}"));
            while let Ok(command) = rx.recv() {
                if matches!(command, Command::Shutdown) {
                    break;
                }
            }
            return;
        }
    };

    let sink = match Sink::try_new(&handle) {
        Ok(sink) => sink,
        Err(error) => {
            shared.fail(format!("无法创建播放通道：{error}"));
            while let Ok(command) = rx.recv() {
                if matches!(command, Command::Shutdown) {
                    break;
                }
            }
            return;
        }
    };
    sink.set_volume(0.85);

    while let Ok(command) = rx.recv() {
        match command {
            Command::Load(bytes) => load(&sink, &shared, bytes),
            Command::Play => {
                if matches!(shared.state(), State::Paused) {
                    sink.play();
                    shared.set_state(State::Playing);
                }
            }
            Command::Pause => {
                if shared.state() == State::Playing {
                    sink.pause();
                    shared.set_state(State::Paused);
                }
            }
            Command::Toggle => match shared.state() {
                State::Playing => {
                    sink.pause();
                    shared.set_state(State::Paused);
                }
                State::Paused => {
                    sink.play();
                    shared.set_state(State::Playing);
                }
                _ => {}
            },
            Command::Stop => {
                sink.stop();
                shared.set_state(State::Idle);
                shared.reset_spectrum();
            }
            Command::Seek(position) => {
                if matches!(shared.state(), State::Playing | State::Paused)
                    && sink.try_seek(position).is_ok()
                {
                    shared
                        .position_ms
                        .store(position.as_millis() as u64, Ordering::Relaxed);
                }
            }
            Command::Volume(volume) => sink.set_volume(volume),
            Command::Shutdown => {
                sink.stop();
                break;
            }
        }
    }

    drop(stream);
}

fn load(sink: &Sink, shared: &Arc<Shared>, bytes: Vec<u8>) {
    sink.stop();
    shared.reset_spectrum();

    // 网易云的 mp3 / flac 继续交给 rodio；B 站的 DASH 音轨（fMP4）需要
    // 自己构造 symphonia 数据源，否则 rodio 会因为报不出文件长度而解码失败。
    let source: Box<dyn Source<Item = f32> + Send> = if crate::mp4::is_mp4(&bytes) {
        match crate::mp4::Mp4Source::open(bytes) {
            Ok(source) => Box::new(source),
            Err(error) => {
                shared.fail(format!("无法解码这首歌曲：{error}"));
                return;
            }
        }
    } else {
        match Decoder::new(Cursor::new(bytes)) {
            Ok(decoder) => Box::new(decoder.convert_samples::<f32>()),
            Err(error) => {
                shared.fail(format!("无法解码这首歌曲：{error}"));
                return;
            }
        }
    };

    let channels = source.channels() as u32;
    let sample_rate = source.sample_rate();
    let duration = source.total_duration().unwrap_or_default();

    shared.channels.store(channels, Ordering::Relaxed);
    shared.sample_rate.store(sample_rate, Ordering::Relaxed);
    shared
        .duration_ms
        .store(duration.as_millis() as u64, Ordering::Relaxed);
    *shared.error.lock().unwrap() = None;

    sink.append(Tap::new(source, Arc::clone(shared)));
    sink.play();
    shared.set_state(State::Playing);
}

/// 原样转发采样，同时写进环形缓冲并统计播放位置。
///
/// 位置按"帧"（每声道一个采样）累计；如果直接拿采样数除以采样率，
/// 立体声会让进度快一倍。
struct Tap<I> {
    inner: I,
    shared: Arc<Shared>,
    channels: u16,
    sample_rate: u32,
    base_ms: u64,
    frames: u64,
    reported: u64,
    /// 当前帧已收到的采样数（0..channels）。
    in_frame: u16,
    pending: Vec<f32>,
    finished: bool,
}

impl<I> Tap<I>
where
    I: Source<Item = f32>,
{
    fn new(inner: I, shared: Arc<Shared>) -> Self {
        let channels = inner.channels().max(1);
        let sample_rate = inner.sample_rate().max(1);
        Self {
            inner,
            shared,
            channels,
            sample_rate,
            base_ms: 0,
            frames: 0,
            reported: 0,
            in_frame: 0,
            pending: Vec::with_capacity(FLUSH_SAMPLES),
            finished: false,
        }
    }

    fn flush(&mut self) {
        if !self.pending.is_empty() {
            self.shared.push_samples(&self.pending);
            self.pending.clear();
        }
    }

    fn report_position(&mut self) {
        if self.frames - self.reported < POSITION_FRAMES && self.frames != 0 {
            return;
        }
        self.reported = self.frames;
        let elapsed = self.frames * 1000 / u64::from(self.sample_rate);
        self.shared
            .position_ms
            .store(self.base_ms + elapsed, Ordering::Relaxed);
    }
}

impl<I> Iterator for Tap<I>
where
    I: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        match self.inner.next() {
            Some(sample) => {
                self.pending.push(sample);
                if self.pending.len() >= FLUSH_SAMPLES {
                    self.flush();
                }

                // 每收齐一个声道就推进一帧。
                self.in_frame += 1;
                if self.in_frame >= self.channels {
                    self.in_frame = 0;
                    self.frames += 1;
                    self.report_position();
                }

                Some(sample)
            }
            None => {
                self.flush();
                if !self.finished {
                    self.finished = true;
                    if self.frames > 0 {
                        self.reported = self.frames;
                        let elapsed = self.frames * 1000 / u64::from(self.sample_rate);
                        self.shared
                            .position_ms
                            .store(self.base_ms + elapsed, Ordering::Relaxed);
                    }
                    self.shared.set_state(State::Finished);
                }
                None
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<I> Source for Tap<I>
where
    I: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.channels
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), rodio::source::SeekError> {
        self.inner.try_seek(position)?;
        self.pending.clear();
        self.base_ms = position.as_millis() as u64;
        self.frames = 0;
        self.reported = 0;
        self.in_frame = 0;
        self.finished = false;
        // 只清采样与进度，时长仍然有效。
        self.shared.reset_samples();
        self.shared
            .position_ms
            .store(self.base_ms, Ordering::Relaxed);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_keeps_the_newest_samples_in_order() {
        let mut ring = Ring::default();
        for value in 0..(RING_SIZE + 100) {
            ring.push(value as f32);
        }
        let snapshot = ring.snapshot();
        assert_eq!(snapshot.len(), RING_SIZE);
        assert_eq!(snapshot[0], 100.0);
        assert_eq!(*snapshot.last().unwrap(), (RING_SIZE + 99) as f32);
    }

    #[test]
    fn ring_reset_clears_everything() {
        let mut ring = Ring::default();
        ring.push(1.0);
        ring.reset();
        assert!(ring.snapshot().is_empty());
    }

    #[test]
    fn ring_batch_keeps_the_newest_samples_in_order() {
        let mut ring = Ring::default();
        let batch: Vec<f32> = (0..(RING_SIZE + 300)).map(|value| value as f32).collect();
        ring.push_batch(&batch);
        let snapshot = ring.snapshot();
        assert_eq!(snapshot.len(), RING_SIZE);
        assert_eq!(snapshot[0], 300.0);
        assert_eq!(*snapshot.last().unwrap(), (RING_SIZE + 299) as f32);
    }

    struct TestSource {
        remaining: u64,
        channels: u16,
        rate: u32,
    }

    impl Iterator for TestSource {
        type Item = f32;

        fn next(&mut self) -> Option<f32> {
            if self.remaining == 0 {
                None
            } else {
                self.remaining -= 1;
                Some(0.25)
            }
        }
    }

    impl Source for TestSource {
        fn current_frame_len(&self) -> Option<usize> {
            None
        }

        fn channels(&self) -> u16 {
            self.channels
        }

        fn sample_rate(&self) -> u32 {
            self.rate
        }

        fn total_duration(&self) -> Option<Duration> {
            None
        }

        fn try_seek(&mut self, _: Duration) -> Result<(), rodio::source::SeekError> {
            Ok(())
        }
    }

    #[test]
    fn stereo_frames_advance_at_real_time() {
        // 44.1kHz 立体声：1 秒 = 88200 个交错采样 = 44100 帧。
        for (channels, samples) in [(2, 88_200u64), (1, 44_100)] {
            let shared = Arc::new(Shared::default());
            let source = TestSource {
                remaining: samples,
                channels,
                rate: 44_100,
            };
            let mut tap = Tap::new(source, Arc::clone(&shared));
            while tap.next().is_some() {}

            let position = shared.position_ms.load(Ordering::Relaxed);
            assert_eq!(position, 1_000, "{channels} 声道时进度异常：{position}ms");
            assert_eq!(shared.state(), State::Finished);
        }
    }

    #[test]
    fn seek_keeps_the_known_duration() {
        let shared = Arc::new(Shared::default());
        shared.duration_ms.store(300_000, Ordering::Relaxed);
        let source = TestSource {
            remaining: 8_820,
            channels: 2,
            rate: 44_100,
        };
        let mut tap = Tap::new(source, Arc::clone(&shared));
        tap.try_seek(Duration::from_secs(120)).unwrap();

        assert_eq!(shared.duration_ms.load(Ordering::Relaxed), 300_000);
        assert_eq!(shared.position_ms.load(Ordering::Relaxed), 120_000);
    }
}

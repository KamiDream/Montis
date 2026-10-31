//! 需要自定义数据源的音频解码（B 站 fMP4 / 网易云 FLAC）。
//!
//! rodio 0.19 的 `Decoder` 会用 `ReadSeekSource` 包装字节流，而它的
//! `byte_len()` 永远返回 `None`：isomp4 解析器初始化时会直接报
//! `SeekError(Unseekable)`，FLAC 的 symphonia 后端也会因此无法二分定位。
//! 无损 / Hi-Res 返回的正是 FLAC，所以拖动进度条不会跳转。
//!
//! 这里复用 symphonia 的解码流程，只把底层数据源换成一个能报告长度的
//! `MediaSource`，并实现成 rodio 的 `Source` 交给播放引擎。

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::time::Duration;

use rodio::source::{SeekError, Source};
use symphonia::core::audio::{Channels, SampleBuffer, SignalSpec};
use symphonia::core::codecs::{Decoder as SymphoniaDecoder, DecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use symphonia::core::units::{Duration as Units, Time};

/// 数据是不是 MP4 / fMP4（`ftyp` box 紧跟在 4 字节长度之后）。
pub fn is_mp4(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[4..8] == b"ftyp"
}

/// 数据是不是原生 FLAC（以 `fLaC` 魔数开头）。
pub fn is_flac(bytes: &[u8]) -> bool {
    bytes.starts_with(b"fLaC")
}

/// 带长度信息的只读字节源：这是绕开 rodio `byte_len = None` 的关键。
struct BytesSource {
    cursor: Cursor<Vec<u8>>,
    len: u64,
}

impl Read for BytesSource {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.cursor.read(buffer)
    }
}

impl Seek for BytesSource {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.cursor.seek(position)
    }
}

impl MediaSource for BytesSource {
    fn is_seekable(&self) -> bool {
        true
    }

    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}

/// 把一整段音频（fMP4 / FLAC 等）解成 f32 采样。
pub struct SymphoniaSource {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn SymphoniaDecoder>,
    track_id: u32,
    spec: SignalSpec,
    buffer: SampleBuffer<f32>,
    position: usize,
    finished: bool,
    duration: Option<Duration>,
}

impl SymphoniaSource {
    pub fn open(bytes: Vec<u8>, extension: &str) -> Result<Self, String> {
        let len = bytes.len() as u64;
        let stream = MediaSourceStream::new(
            Box::new(BytesSource {
                cursor: Cursor::new(bytes),
                len,
            }),
            Default::default(),
        );

        let mut hint = Hint::new();
        hint.with_extension(extension);

        let probed = symphonia::default::get_probe()
            .format(
                &hint,
                stream,
                &FormatOptions {
                    enable_gapless: true,
                    ..Default::default()
                },
                &MetadataOptions::default(),
            )
            .map_err(|error| format!("无法解析音频容器：{error}"))?;

        let track = probed
            .format
            .default_track()
            .or_else(|| probed.format.tracks().first())
            .ok_or_else(|| "音频文件没有可用轨道。".to_string())?;

        let track_id = track.id;
        let params = track.codec_params.clone();
        let decoder = symphonia::default::get_codecs()
            .make(&params, &DecoderOptions::default())
            .map_err(|error| format!("无法创建音频解码器：{error}"))?;

        let channels = params
            .channels
            .filter(|channels| !channels.is_empty())
            .unwrap_or(Channels::FRONT_LEFT | Channels::FRONT_RIGHT);
        let sample_rate = params.sample_rate.unwrap_or(44_100);
        let spec = SignalSpec::new(sample_rate, channels);
        let duration = params.time_base.zip(params.n_frames).map(|(base, frames)| {
            let time = base.calc_time(frames);
            Duration::from_secs_f64(time.seconds as f64 + time.frac)
        });

        Ok(Self {
            format: probed.format,
            decoder,
            track_id,
            spec,
            buffer: SampleBuffer::<f32>::new(Units::from(0u64), spec),
            position: 0,
            finished: false,
            duration,
        })
    }

    /// 解出下一个音频包；返回 false 表示已经到文件末尾或无法继续。
    fn fill(&mut self) -> bool {
        loop {
            let packet = match self.format.next_packet() {
                Ok(packet) => packet,
                Err(_) => {
                    self.finished = true;
                    return false;
                }
            };

            if packet.track_id() != self.track_id {
                continue;
            }

            match self.decoder.decode(&packet) {
                Ok(decoded) => {
                    self.spec = decoded.spec().to_owned();
                    let mut buffer = SampleBuffer::<f32>::new(
                        Units::from(decoded.capacity() as u64),
                        *decoded.spec(),
                    );
                    buffer.copy_interleaved_ref(decoded);
                    self.buffer = buffer;
                    self.position = 0;
                    return true;
                }
                // 单个坏包不值得中断整首歌，跳过即可。
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(_) => {
                    self.finished = true;
                    return false;
                }
            }
        }
    }
}

impl Iterator for SymphoniaSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        loop {
            if self.position < self.buffer.samples().len() {
                let sample = self.buffer.samples()[self.position];
                self.position += 1;
                return Some(sample);
            }

            if self.finished || !self.fill() {
                return None;
            }
        }
    }
}

impl Source for SymphoniaSource {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        self.spec.channels.count() as u16
    }

    fn sample_rate(&self) -> u32 {
        self.spec.rate
    }

    fn total_duration(&self) -> Option<Duration> {
        self.duration
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        let time = Time::new(
            position.as_secs(),
            position.subsec_nanos() as f64 / 1_000_000_000.0,
        );

        self.format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|_| SeekError::NotSupported {
                underlying_source: "montis-symphonia",
            })?;

        self.decoder.reset();
        self.buffer = SampleBuffer::<f32>::new(Units::from(0u64), self.spec);
        self.position = 0;
        self.finished = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{is_flac, is_mp4};

    #[test]
    fn detects_mp4_signature() {
        let mut bytes = vec![0u8; 16];
        bytes[4..8].copy_from_slice(b"ftyp");
        assert!(is_mp4(&bytes));
        assert!(!is_mp4(b"ID3\x04\x00\x00\x00\x00\x00\x00\x00\x00"));
        assert!(!is_mp4(b"short"));
    }

    #[test]
    fn detects_flac_signature() {
        assert!(is_flac(b"fLaC\x00\x00\x00\x00"));
        assert!(!is_flac(b"ID3\x04\x00\x00"));
        assert!(!is_flac(b"OggS"));
    }

    /// 手工验证：`cargo test -p nm-audio --offline --lib -- --ignored --nocapture`
    #[test]
    #[ignore = "需要 /tmp/bili_sample.m4s"]
    fn decodes_bilibili_dash_sample() {
        use rodio::Source;

        let bytes = std::fs::read("/tmp/bili_sample.m4s").expect("sample");
        let mut source = super::SymphoniaSource::open(bytes, "m4a").expect("open mp4");
        assert!(source.total_duration().is_some(), "没有读到时长");

        let samples = source.by_ref().take(44_100 * 3).count();
        println!("decoded {samples} samples");
        assert!(samples > 0, "没有解码出任何采样");

        source
            .try_seek(Duration::from_secs(30))
            .expect("seek to 30s");
        assert!(source.next().is_some(), "seek 后没有音频");
        println!("seek ok, duration = {:?}", source.total_duration());
    }
}

//! 频谱分析：频段划分 / FFT / 平滑。
//!
//! 逻辑与 VisualSpectrum（本机另一个项目）保持一致：对数频段、分区域自动增益、
//! 攻击/释放平滑。这里只保留播放器需要的部分，不碰音频设备。

use std::f64::consts::PI;

use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

/// FFT 长度。2048 @ 44.1kHz ≈ 46ms 一帧，频率分辨率 ≈ 21Hz。
pub const FFT_SIZE: usize = 2_048;

/// 频段覆盖范围（Hz）。低于 30Hz 基本是直流/噪声，高于 ~16kHz 人耳听不到。
pub const F_MIN: f64 = 30.0;
pub const F_MAX: f64 = 16_000.0;

/// 低 / 中 / 高音的分界（占频段数的比例）。
pub const REGION_SPLIT: (f64, f64) = (0.34, 0.78);

/// 把 rfft 的输出（长 `fft_size/2 + 1`）按**对数**切成 `count` 段，返回每段 `[lo, hi)`。
///
/// 音乐是八度的，不是线性的：低频窄、高频宽。每段至少 1 个 bin。
pub fn band_bins(
    count: usize,
    sample_rate: usize,
    fft_size: usize,
    f_min: f64,
    f_max: f64,
) -> Vec<(usize, usize)> {
    assert!(count >= 1, "count 至少要 1");
    let bins = fft_size / 2 + 1;
    let nyquist = sample_rate as f64 / 2.0;
    let mut hi_hz = f_max.min(nyquist * 0.98);
    let lo_hz = f_min.max(20.0);
    if hi_hz <= lo_hz {
        hi_hz = lo_hz * 2.0;
    }

    let ratio = hi_hz / lo_hz;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let lo_hz_i = lo_hz * ratio.powf(i as f64 / count as f64);
        let hi_hz_i = lo_hz * ratio.powf((i + 1) as f64 / count as f64);
        let lo = (lo_hz_i / nyquist * (bins - 1) as f64).round() as isize;
        let hi = (hi_hz_i / nyquist * (bins - 1) as f64).round() as isize;
        let lo = lo.clamp(0, bins as isize - 1) as usize;
        let hi = hi.max(lo as isize + 1).min(bins as isize) as usize;
        out.push((lo, hi));
    }
    out
}

/// 把每个频段里的幅度取平均。
pub fn map_bands(magnitudes: &[f64], ranges: &[(usize, usize)]) -> Vec<f64> {
    ranges
        .iter()
        .map(|&(lo, hi)| {
            let hi = hi.min(magnitudes.len());
            let lo = lo.min(hi);
            let segment = &magnitudes[lo..hi];
            if segment.is_empty() {
                0.0
            } else {
                segment.iter().sum::<f64>() / segment.len() as f64
            }
        })
        .collect()
}

/// 对一帧**已混成单路**的采样做 FFT 并聚合成频段幅度。
///
/// 不足 `fft_size` 尾部补零，超过取最后一截；Hann 窗按 `N-1` 归一。
/// 适合一次性调用；逐帧分析请用 [`Analyzer`]（会复用 FFT 计划与缓冲区）。
pub fn bands_of(samples: &[f32], fft_size: usize, ranges: &[(usize, usize)]) -> Vec<f64> {
    let mut planner = FftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(fft_size);
    let mut buffer = vec![Complex::new(0.0, 0.0); fft_size];
    let mut out = vec![0.0; ranges.len()];
    analyze(&mut buffer, &*fft, samples, fft_size, ranges, &mut out);
    out
}

/// 真正做窗函数 + FFT + 频段聚合的核心。
fn analyze(
    buffer: &mut [Complex<f64>],
    fft: &dyn rustfft::Fft<f64>,
    samples: &[f32],
    fft_size: usize,
    ranges: &[(usize, usize)],
    out: &mut [f64],
) {
    let denom = fft_size.saturating_sub(1).max(1) as f64;
    let longer = samples.len() > fft_size;
    let start = samples.len().saturating_sub(fft_size);

    for (i, slot) in buffer.iter_mut().enumerate() {
        let sample = if longer {
            samples[start + i] as f64
        } else if i < samples.len() {
            samples[i] as f64
        } else {
            0.0
        };
        let window = 0.5 - 0.5 * (2.0 * PI * i as f64 / denom).cos();
        *slot = Complex::new(sample * window, 0.0);
    }

    fft.process(buffer);

    let bins = fft_size / 2 + 1;
    for (band, &(lo, hi)) in ranges.iter().enumerate() {
        let hi = hi.min(bins);
        let lo = lo.min(hi);
        let segment = &buffer[lo..hi];
        out[band] = if segment.is_empty() {
            0.0
        } else {
            segment.iter().map(|value| value.norm()).sum::<f64>() / segment.len() as f64
        };
    }
}

/// 逐帧分析器：缓存 FFT 计划、频段划分与所有缓冲区。
///
/// 播放时这个函数每秒跑几十次，不能每次都新建 `FftPlanner`。
pub struct Analyzer {
    count: usize,
    sample_rate: usize,
    fft_size: usize,
    ranges: Vec<(usize, usize)>,
    fft: std::sync::Arc<dyn rustfft::Fft<f64>>,
    buffer: Vec<Complex<f64>>,
    mono: Vec<f32>,
    out: Vec<f64>,
}

impl Analyzer {
    pub fn new(count: usize, sample_rate: usize, fft_size: usize) -> Self {
        let sample_rate = sample_rate.max(8_000);
        let mut planner = FftPlanner::<f64>::new();
        let fft = planner.plan_fft_forward(fft_size);

        Self {
            count,
            sample_rate,
            fft_size,
            ranges: band_bins(count, sample_rate, fft_size, F_MIN, F_MAX),
            fft,
            buffer: vec![Complex::new(0.0, 0.0); fft_size],
            mono: Vec::with_capacity(fft_size),
            out: vec![0.0; count],
        }
    }

    pub fn set_sample_rate(&mut self, sample_rate: usize) {
        let sample_rate = sample_rate.max(8_000);
        if sample_rate != self.sample_rate {
            self.sample_rate = sample_rate;
            self.ranges = band_bins(self.count, sample_rate, self.fft_size, F_MIN, F_MAX);
        }
    }

    /// 输入交错采样，返回已聚合的频段幅度（复用内部缓冲）。
    pub fn bands(&mut self, samples: &[f32], channels: usize) -> &[f64] {
        let channels = channels.max(1);
        let frames = samples.len() / channels;

        self.mono.clear();
        if channels == 1 {
            self.mono.extend_from_slice(&samples[..frames]);
        } else {
            self.mono.reserve(frames);
            for frame in 0..frames {
                let base = frame * channels;
                let sum: f32 = samples[base..base + channels].iter().sum();
                self.mono.push(sum / channels as f32);
            }
        }

        analyze(
            &mut self.buffer,
            &*self.fft,
            &self.mono,
            self.fft_size,
            &self.ranges,
            &mut self.out,
        );
        &self.out
    }
}

/// 交错立体声 → 先平均成单路，再做 [`bands_of`]。平均比只取第一路稳。
pub fn bands_of_interleaved(
    samples: &[f32],
    channels: usize,
    fft_size: usize,
    ranges: &[(usize, usize)],
) -> Vec<f64> {
    if channels <= 1 {
        return bands_of(samples, fft_size, ranges);
    }
    let frames = samples.len() / channels;
    let mut mono = Vec::with_capacity(frames);
    for frame in 0..frames {
        let base = frame * channels;
        let sum: f32 = samples[base..base + channels].iter().sum();
        mono.push(sum / channels as f32);
    }
    bands_of(&mono, fft_size, ranges)
}

/// 把 `count` 个频段切成低/中/高三块，返回 `[(lo, hi, 区域号)]`。
pub fn region_bounds(count: usize) -> Vec<(usize, usize, usize)> {
    if count < 3 {
        return vec![(0, count, 0)];
    }
    let (lo_split, hi_split) = REGION_SPLIT;
    let b1 = ((count as f64 * lo_split).round() as usize)
        .max(1)
        .min(count - 2);
    let b2 = ((count as f64 * hi_split).round() as usize)
        .max(b1 + 1)
        .min(count - 1);
    vec![(0, b1, 0), (b1, b2, 1), (b2, count, 2)]
}

/// 把原始幅度变成"好看、稳定"的 0~1 电平。
///
/// 分区域自动增益 → 占比增益 → 伽马 → 攻击/释放平滑。分区域是为了避免低频
/// 永远顶格：低频通常最响，用全局峰值做参考线会把它压平。
#[derive(Debug, Clone)]
pub struct SpectrumProcessor {
    pub count: usize,
    pub attack: f64,
    pub release: f64,
    pub gamma: f64,
    /// 参考线回落速度（每帧向当前峰值靠拢多少）—— 越小越"长记忆"。
    pub ref_falloff: f64,
    pub gains: [f64; 3],
    regions: Vec<(usize, usize, usize)>,
    global_ref: f64,
    refs: [f64; 3],
    values: Vec<f64>,
}

impl SpectrumProcessor {
    pub fn new(count: usize, gains: [f64; 3]) -> Self {
        Self::with_params(count, 0.75, 0.30, 0.75, 0.05, gains)
    }

    pub fn with_params(
        count: usize,
        attack: f64,
        release: f64,
        gamma: f64,
        ref_falloff: f64,
        gains: [f64; 3],
    ) -> Self {
        Self {
            count,
            attack,
            release,
            gamma,
            ref_falloff,
            gains,
            regions: region_bounds(count),
            global_ref: 1e-6,
            refs: [1e-6; 3],
            values: vec![0.0; count],
        }
    }

    pub fn set_gains(&mut self, gains: [f64; 3]) {
        self.gains = gains;
    }

    pub fn reset(&mut self, count: Option<usize>) {
        if let Some(count) = count {
            self.count = count;
            self.regions = region_bounds(count);
        }
        self.global_ref = 1e-6;
        self.refs = [1e-6; 3];
        self.values = vec![0.0; self.count];
    }

    /// 输入原始线性幅度，返回等长的 0~1 电平（就地更新内部状态）。
    pub fn process(&mut self, bands: &[f64]) -> Vec<f64> {
        if bands.len() != self.count {
            self.reset(Some(bands.len()));
        }
        let mut out = vec![0.0; self.count];
        self.process_into(bands, &mut out);
        out
    }

    /// 同 [`Self::process`]，但把结果写进复用缓冲区，避免每帧分配。
    pub fn process_into(&mut self, bands: &[f64], out: &mut Vec<f64>) {
        if bands.len() != self.count {
            self.reset(Some(bands.len()));
        }
        if out.len() != self.count {
            out.resize(self.count, 0.0);
        }

        let falloff = self.ref_falloff;
        let peak_all = bands.iter().copied().fold(0.0_f64, f64::max);
        self.global_ref = if peak_all > self.global_ref {
            peak_all
        } else {
            self.global_ref + (peak_all - self.global_ref) * falloff
        };
        let floor = (self.global_ref * 0.05).max(1e-6);

        for &(lo, hi, region) in &self.regions {
            let hi = hi.min(bands.len());
            let lo = lo.min(hi);
            let peak = bands[lo..hi].iter().copied().fold(0.0_f64, f64::max);
            let mut reference = self.refs[region];
            reference = if peak > reference {
                peak
            } else {
                reference + (peak - reference) * falloff
            };
            reference = reference.max(floor);
            self.refs[region] = reference;

            let gain = self.gains[region] / reference;
            for i in lo..hi {
                let x = (bands[i] * gain).clamp(0.0, 1.0).powf(self.gamma);
                let prev = self.values[i];
                let k = if x > prev { self.attack } else { self.release };
                let y = prev + (x - prev) * k;
                self.values[i] = y;
                out[i] = y;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bands_cover_the_spectrum_without_gaps() {
        let ranges = band_bins(48, 44_100, FFT_SIZE, F_MIN, F_MAX);
        assert_eq!(ranges.len(), 48);
        assert!(ranges[0].0 < ranges[0].1);
        for pair in ranges.windows(2) {
            // 低频段很窄，取整后可能共享一个 bin，所以允许 1 个 bin 的重叠。
            assert!(pair[0].0 <= pair[1].0, "频段应单调递增");
            assert!(pair[0].1 <= pair[1].0 + 1, "相邻频段重叠过多");
        }
        assert!(ranges.last().unwrap().1 <= FFT_SIZE / 2 + 1);
    }

    #[test]
    fn low_frequencies_stay_in_the_first_bands() {
        let sample_rate = 44_100;
        let ranges = band_bins(24, sample_rate, FFT_SIZE, F_MIN, F_MAX);
        // 1kHz 正弦波的能量应落在下半个频谱（24 段时大约第 13 段）。
        let samples: Vec<f32> = (0..FFT_SIZE)
            .map(|i| (2.0 * PI * 1_000.0 * i as f64 / sample_rate as f64).sin() as f32)
            .collect();
        let bands = bands_of(&samples, FFT_SIZE, &ranges);
        let peak = bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert!(peak < 16, "1kHz 的峰值落到了第 {peak} 段");
    }

    #[test]
    fn interleaved_stereo_is_mixed_down() {
        let ranges = band_bins(8, 48_000, FFT_SIZE, F_MIN, F_MAX);
        let mono: Vec<f32> = (0..FFT_SIZE).map(|i| (i as f32 * 0.01).sin()).collect();
        let mut interleaved = Vec::with_capacity(FFT_SIZE * 2);
        for sample in &mono {
            interleaved.push(*sample);
            interleaved.push(*sample);
        }
        let from_stereo = bands_of_interleaved(&interleaved, 2, FFT_SIZE, &ranges);
        let from_mono = bands_of(&mono, FFT_SIZE, &ranges);
        for (a, b) in from_stereo.iter().zip(from_mono.iter()) {
            assert!((a - b).abs() < 1e-9);
        }
    }

    #[test]
    fn processor_stays_in_unit_range() {
        let mut processor = SpectrumProcessor::new(32, [1.0, 1.0, 1.0]);
        let mut bands = vec![0.0; 32];
        bands[3] = 20.0;
        bands[15] = 5.0;
        let out = processor.process(&bands);
        assert_eq!(out.len(), 32);
        for value in &out {
            assert!((0.0..=1.0).contains(value));
        }
        assert!(out[3] > out[20]);
    }

    #[test]
    fn processor_smooths_release() {
        let mut processor = SpectrumProcessor::new(8, [1.0, 1.0, 1.0]);
        processor.process(&[1.0; 8]);
        let out = processor.process(&[0.0; 8]);
        assert!(out.iter().all(|value| *value > 0.0), "释放应当有拖尾");
    }
}

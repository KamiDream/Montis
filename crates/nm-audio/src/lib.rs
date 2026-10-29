//! Montis（山音）的音频层：频谱分析与播放引擎。
//!
//! * [`dsp`]：纯函数 + 平滑器，输入采样输出 0~1 的频段电平；
//! * [`player`]：专用线程里的 rodio 播放器，主线程通过句柄控制。

pub mod dsp;
pub mod player;

pub use dsp::{
    band_bins, bands_of_interleaved, Analyzer, SpectrumProcessor, FFT_SIZE, F_MAX, F_MIN,
};
pub use player::{Engine, Snapshot, State};

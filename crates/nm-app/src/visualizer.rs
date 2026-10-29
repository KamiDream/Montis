//! 环形频谱的可调参数、配色与持久化。
//!
//! 参考 VisualSpectrum 的配置：条数、环大小 / 条高、强度、低中高增益、自转、
//! 顶端连线、辉光、中心辉光、余晖、CRT（扫描线 / 桶形 / 色散）、粒子与
//! 高刷新率。改动即时生效并写入 `~/.config/melody/visualizer.json`。

use std::path::Path;

use serde::{Deserialize, Serialize};

// ==========================================================================
// 取值区间
// ==========================================================================

// 取值范围与 VisualSpectrum 对齐（不复刻它更窄的 UI 限制）：
// 环大小可以超过 100%（溢出舞台）、条高可以超过 100%（从圆心往外长），
// 强度 / 增益也保留了参考项目的上限。
pub const BARS_RANGE: (u32, u32) = (4, 512);
pub const SIZE_RANGE: (f32, f32) = (0.05, 2.0);
pub const HEIGHT_RANGE: (f32, f32) = (0.05, 5.0);
pub const INTENSITY_RANGE: (f32, f32) = (0.1, 10.0);
pub const SPEED_RANGE: (f32, f32) = (0.1, 3.0);
pub const GLOW_RANGE: (f32, f32) = (0.0, 2.0);
pub const CENTER_GLOW_RANGE: (f32, f32) = (0.0, 2.0);
pub const AFTERGLOW_RANGE: (f32, f32) = (0.0, 1.0);
pub const SENSITIVITY_RANGE: (f32, f32) = (0.0, 1.0);
/// CRT 后处理的三个分量（扫描线 / 桶形畸变 / RGB 色散）。
pub const CRT_RANGE: (f32, f32) = (0.0, 1.0);
/// 低 / 中 / 高音增益。
pub const GAIN_RANGE: (f32, f32) = (0.0, 4.0);
/// 粒子速度 / 寿命倍数。
pub const PARTICLE_RANGE: (f32, f32) = (0.2, 4.0);
/// 音条持续粒子的密度倍数（0 = 不发射）。
pub const PARTICLE_AMOUNT_RANGE: (f32, f32) = (0.0, 2.5);

// ==========================================================================
// 配色
// ==========================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Palette {
    #[default]
    Neon,
    Sunset,
    Ice,
    /// 品牌配色；旧配置里叫 `melo`，保留别名以兼容。
    #[serde(rename = "montis", alias = "melo")]
    Montis,
    Forest,
    Ember,
    Aurora,
    Mono,
}

impl Palette {
    pub const ALL: [Palette; 8] = [
        Palette::Neon,
        Palette::Sunset,
        Palette::Ice,
        Palette::Montis,
        Palette::Forest,
        Palette::Ember,
        Palette::Aurora,
        Palette::Mono,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Palette::Neon => "霓虹",
            Palette::Sunset => "落日",
            Palette::Ice => "冰蓝",
            Palette::Montis => "Montis",
            Palette::Forest => "森林",
            Palette::Ember => "焰火",
            Palette::Aurora => "极光",
            Palette::Mono => "素白",
        }
    }

    pub fn colors(self) -> [[f32; 3]; 3] {
        match self {
            Palette::Neon => [
                [0.494, 0.906, 1.0],
                [0.702, 0.482, 1.0],
                [1.0, 0.435, 0.847],
            ],
            Palette::Sunset => [
                [1.0, 0.820, 0.400],
                [1.0, 0.482, 0.353],
                [0.765, 0.298, 1.0],
            ],
            Palette::Ice => [
                [0.659, 0.941, 1.0],
                [0.498, 0.718, 1.0],
                [0.612, 0.549, 1.0],
            ],
            Palette::Montis => [
                [0.231, 0.447, 1.0],
                [0.541, 0.416, 1.0],
                [0.408, 0.839, 1.0],
            ],
            Palette::Forest => [
                [0.486, 1.0, 0.698],
                [0.227, 0.820, 0.627],
                [0.173, 0.549, 0.600],
            ],
            Palette::Ember => [
                [1.0, 0.820, 0.400],
                [1.0, 0.420, 0.208],
                [0.843, 0.149, 0.239],
            ],
            Palette::Aurora => [
                [0.388, 0.961, 0.784],
                [0.431, 0.776, 1.0],
                [0.702, 0.533, 1.0],
            ],
            Palette::Mono => [
                [1.0, 1.0, 1.0],
                [0.788, 0.839, 0.890],
                [0.529, 0.588, 0.659],
            ],
        }
    }

    /// 在色标上取色，`t` 为 0~1。
    pub fn color_at(self, t: f32) -> [f32; 3] {
        let colors = self.colors();
        let t = t.clamp(0.0, 1.0) * (colors.len() - 1) as f32;
        let lo = t.floor() as usize;
        let hi = (lo + 1).min(colors.len() - 1);
        let k = t - lo as f32;
        let a = colors[lo];
        let b = colors[hi];
        [
            a[0] + (b[0] - a[0]) * k,
            a[1] + (b[1] - a[1]) * k,
            a[2] + (b[2] - a[2]) * k,
        ]
    }
}

// ==========================================================================
// 配置
// ==========================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VisualizerConfig {
    /// 环上的频谱条数。
    pub bars: u32,
    /// 环外径占窗口短边一半的比例。
    pub size: f32,
    /// 条长占环半径的比例。
    pub height: f32,
    /// 电平放大倍数。
    pub intensity: f32,
    /// 低 / 中 / 高音的增益。
    pub bass: f32,
    pub mid: f32,
    pub treble: f32,
    pub rotate: bool,
    pub rotate_speed: f32,
    /// 相邻条顶端连线。
    pub links: bool,
    /// 条本身的辉光强度（0 = 关闭）。
    pub glow: f32,
    /// 环心光晕强度。
    pub center_glow: f32,
    /// 余晖拖尾（0 = 无拖尾）。
    pub afterglow: f32,
    /// CRT 后处理强度（桶形畸变 + RGB 色散的总开关）。
    pub crt: f32,
    /// 桶形畸变（CRT 画面外鼓）。
    pub barrel: f32,
    /// RGB 色散。
    pub aberration: f32,
    pub particles: bool,
    /// 音条持续粒子的密度倍数。
    pub particle_amount: f32,
    /// 鼓点爆发开关（与持续粒子独立）。
    pub burst: bool,
    /// 鼓点灵敏度。
    pub sensitivity: f32,
    /// 粒子速度 / 寿命倍数。
    pub particle_speed: f32,
    pub particle_life: f32,
    /// 鼓点爆发的速度 / 寿命倍数。
    pub burst_speed: f32,
    pub burst_life: f32,
    /// 高刷新率（120fps）；关闭时锁 60fps。
    pub high_fps: bool,
    pub palette: Palette,
}

impl Default for VisualizerConfig {
    fn default() -> Self {
        Self {
            bars: 96,
            size: 0.92,
            height: 0.46,
            intensity: 1.0,
            bass: 1.0,
            mid: 1.0,
            treble: 0.9,
            rotate: true,
            rotate_speed: 1.0,
            links: true,
            glow: 1.0,
            center_glow: 1.0,
            afterglow: 0.60,
            crt: 0.35,
            barrel: 0.45,
            aberration: 0.55,
            particles: true,
            particle_amount: 1.0,
            burst: true,
            sensitivity: 0.55,
            particle_speed: 1.0,
            particle_life: 1.0,
            burst_speed: 1.0,
            burst_life: 1.0,
            high_fps: false,
            palette: Palette::Neon,
        }
    }
}

impl VisualizerConfig {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Self::default();
        };
        // 旧版本只有一个「鼓点粒子」开关；升级后拆成「持续粒子 / 鼓点爆发」，
        // 老配置里没有 `burst` 时继续沿用原来的选择。
        let burst_missing = value.get("burst").is_none();
        match serde_json::from_value::<Self>(value) {
            Ok(mut config) => {
                if burst_missing {
                    config.burst = config.particles;
                }
                config.clamp();
                config
            }
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("序列化设置失败：{error}"))?;
        std::fs::write(path, text).map_err(|error| format!("保存设置失败：{error}"))
    }

    pub fn clamp(&mut self) {
        self.bars = self.bars.clamp(BARS_RANGE.0, BARS_RANGE.1);
        self.size = self.size.clamp(SIZE_RANGE.0, SIZE_RANGE.1);
        self.height = self.height.clamp(HEIGHT_RANGE.0, HEIGHT_RANGE.1);
        self.intensity = self.intensity.clamp(INTENSITY_RANGE.0, INTENSITY_RANGE.1);
        self.bass = self.bass.clamp(GAIN_RANGE.0, GAIN_RANGE.1);
        self.mid = self.mid.clamp(GAIN_RANGE.0, GAIN_RANGE.1);
        self.treble = self.treble.clamp(GAIN_RANGE.0, GAIN_RANGE.1);
        self.rotate_speed = self.rotate_speed.clamp(SPEED_RANGE.0, SPEED_RANGE.1);
        self.glow = self.glow.clamp(GLOW_RANGE.0, GLOW_RANGE.1);
        self.center_glow = self
            .center_glow
            .clamp(CENTER_GLOW_RANGE.0, CENTER_GLOW_RANGE.1);
        self.afterglow = self.afterglow.clamp(AFTERGLOW_RANGE.0, AFTERGLOW_RANGE.1);
        self.crt = self.crt.clamp(CRT_RANGE.0, CRT_RANGE.1);
        self.barrel = self.barrel.clamp(CRT_RANGE.0, CRT_RANGE.1);
        self.aberration = self.aberration.clamp(CRT_RANGE.0, CRT_RANGE.1);
        self.sensitivity = self
            .sensitivity
            .clamp(SENSITIVITY_RANGE.0, SENSITIVITY_RANGE.1);
        self.particle_speed = self
            .particle_speed
            .clamp(PARTICLE_RANGE.0, PARTICLE_RANGE.1);
        self.particle_life = self.particle_life.clamp(PARTICLE_RANGE.0, PARTICLE_RANGE.1);
        self.particle_amount = self
            .particle_amount
            .clamp(PARTICLE_AMOUNT_RANGE.0, PARTICLE_AMOUNT_RANGE.1);
        self.burst_speed = self.burst_speed.clamp(PARTICLE_RANGE.0, PARTICLE_RANGE.1);
        self.burst_life = self.burst_life.clamp(PARTICLE_RANGE.0, PARTICLE_RANGE.1);
    }

    /// 频谱平滑器使用的低 / 中 / 高音增益。
    pub fn gains(&self) -> [f64; 3] {
        [self.bass as f64, self.mid as f64, self.treble as f64]
    }

    /// 某个字段在滑杆上的 0~1 位置。
    pub fn normalized(&self, field: VisualField) -> f32 {
        match field {
            VisualField::Bars => {
                normalize(self.bars as f32, (BARS_RANGE.0 as f32, BARS_RANGE.1 as f32))
            }
            VisualField::Size => normalize(self.size, SIZE_RANGE),
            VisualField::Height => normalize(self.height, HEIGHT_RANGE),
            VisualField::Intensity => normalize(self.intensity, INTENSITY_RANGE),
            VisualField::Bass => normalize(self.bass, GAIN_RANGE),
            VisualField::Mid => normalize(self.mid, GAIN_RANGE),
            VisualField::Treble => normalize(self.treble, GAIN_RANGE),
            VisualField::RotationSpeed => normalize(self.rotate_speed, SPEED_RANGE),
            VisualField::Glow => normalize(self.glow, GLOW_RANGE),
            VisualField::CenterGlow => normalize(self.center_glow, CENTER_GLOW_RANGE),
            VisualField::Afterglow => normalize(self.afterglow, AFTERGLOW_RANGE),
            VisualField::Crt => self.crt,
            VisualField::Barrel => self.barrel,
            VisualField::Aberration => self.aberration,
            VisualField::Sensitivity => normalize(self.sensitivity, SENSITIVITY_RANGE),
            VisualField::ParticleSpeed => normalize(self.particle_speed, PARTICLE_RANGE),
            VisualField::ParticleLife => normalize(self.particle_life, PARTICLE_RANGE),
            VisualField::ParticleAmount => normalize(self.particle_amount, PARTICLE_AMOUNT_RANGE),
            VisualField::BurstSpeed => normalize(self.burst_speed, PARTICLE_RANGE),
            VisualField::BurstLife => normalize(self.burst_life, PARTICLE_RANGE),
        }
    }

    /// 把滑杆的 0~1 位置写回字段。
    pub fn apply_normalized(&mut self, field: VisualField, value: f32) {
        let value = value.clamp(0.0, 1.0);
        match field {
            VisualField::Bars => {
                self.bars = lerp_f32(BARS_RANGE.0 as f32, BARS_RANGE.1 as f32, value).round() as u32
            }
            VisualField::Size => self.size = lerp(SIZE_RANGE, value),
            VisualField::Height => self.height = lerp(HEIGHT_RANGE, value),
            VisualField::Intensity => self.intensity = lerp(INTENSITY_RANGE, value),
            VisualField::Bass => self.bass = lerp(GAIN_RANGE, value),
            VisualField::Mid => self.mid = lerp(GAIN_RANGE, value),
            VisualField::Treble => self.treble = lerp(GAIN_RANGE, value),
            VisualField::RotationSpeed => self.rotate_speed = lerp(SPEED_RANGE, value),
            VisualField::Glow => self.glow = lerp(GLOW_RANGE, value),
            VisualField::CenterGlow => self.center_glow = lerp(CENTER_GLOW_RANGE, value),
            VisualField::Afterglow => self.afterglow = lerp(AFTERGLOW_RANGE, value),
            VisualField::Crt => self.crt = value,
            VisualField::Barrel => self.barrel = value,
            VisualField::Aberration => self.aberration = value,
            VisualField::Sensitivity => self.sensitivity = lerp(SENSITIVITY_RANGE, value),
            VisualField::ParticleSpeed => self.particle_speed = lerp(PARTICLE_RANGE, value),
            VisualField::ParticleLife => self.particle_life = lerp(PARTICLE_RANGE, value),
            VisualField::ParticleAmount => {
                self.particle_amount = lerp(PARTICLE_AMOUNT_RANGE, value)
            }
            VisualField::BurstSpeed => self.burst_speed = lerp(PARTICLE_RANGE, value),
            VisualField::BurstLife => self.burst_life = lerp(PARTICLE_RANGE, value),
        }
        self.clamp();
    }

    /// 面板上显示的数值文案。
    pub fn display(&self, field: VisualField) -> String {
        match field {
            VisualField::Bars => format!("{} 根", self.bars),
            VisualField::Size => format!("{:.0}%", self.size * 100.0),
            VisualField::Height => format!("{:.0}%", self.height * 100.0),
            VisualField::Intensity => format!("{:.1}×", self.intensity),
            VisualField::Bass => format!("{:.1}×", self.bass),
            VisualField::Mid => format!("{:.1}×", self.mid),
            VisualField::Treble => format!("{:.1}×", self.treble),
            VisualField::RotationSpeed => {
                if self.rotate {
                    format!("{:.1}×", self.rotate_speed)
                } else {
                    "关闭".to_string()
                }
            }
            VisualField::Glow => format!("{:.0}%", self.glow * 100.0),
            VisualField::CenterGlow => format!("{:.0}%", self.center_glow * 100.0),
            VisualField::Afterglow => format!("{:.0}%", self.afterglow * 100.0),
            VisualField::Crt => format!("{:.0}%", self.crt * 100.0),
            VisualField::Barrel => format!("{:.0}%", self.barrel * 100.0),
            VisualField::Aberration => format!("{:.0}%", self.aberration * 100.0),
            VisualField::Sensitivity => format!("{:.0}%", self.sensitivity * 100.0),
            VisualField::ParticleSpeed => format!("{:.1}×", self.particle_speed),
            VisualField::ParticleLife => format!("{:.1}×", self.particle_life),
            VisualField::ParticleAmount => format!("{:.0}%", self.particle_amount * 100.0),
            VisualField::BurstSpeed => format!("{:.1}×", self.burst_speed),
            VisualField::BurstLife => format!("{:.1}×", self.burst_life),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualField {
    Bars,
    Size,
    Height,
    Intensity,
    Bass,
    Mid,
    Treble,
    RotationSpeed,
    Glow,
    CenterGlow,
    Afterglow,
    Crt,
    Barrel,
    Aberration,
    Sensitivity,
    ParticleAmount,
    ParticleSpeed,
    ParticleLife,
    BurstSpeed,
    BurstLife,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisualToggle {
    Rotate,
    Links,
    Particles,
    Burst,
    HighFps,
}

fn normalize(value: f32, range: (f32, f32)) -> f32 {
    if range.1 <= range.0 {
        0.0
    } else {
        ((value - range.0) / (range.1 - range.0)).clamp(0.0, 1.0)
    }
}

fn lerp(range: (f32, f32), t: f32) -> f32 {
    lerp_f32(range.0, range.1, t)
}

fn lerp_f32(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_roundtrip_keeps_values_in_range() {
        let mut config = VisualizerConfig::default();
        for field in [
            VisualField::Bars,
            VisualField::Size,
            VisualField::Height,
            VisualField::Intensity,
            VisualField::Bass,
            VisualField::Mid,
            VisualField::Treble,
            VisualField::RotationSpeed,
            VisualField::Glow,
            VisualField::CenterGlow,
            VisualField::Afterglow,
            VisualField::Crt,
            VisualField::Barrel,
            VisualField::Aberration,
            VisualField::Sensitivity,
            VisualField::ParticleAmount,
            VisualField::ParticleSpeed,
            VisualField::ParticleLife,
            VisualField::BurstSpeed,
            VisualField::BurstLife,
        ] {
            config.apply_normalized(field, 1.0);
            let at_max = config.normalized(field);
            assert!((at_max - 1.0).abs() < 1e-5, "{field:?} 最大值异常");

            config.apply_normalized(field, 0.0);
            let at_min = config.normalized(field);
            assert!(at_min < 1e-5, "{field:?} 最小值异常");
        }
    }

    #[test]
    fn out_of_range_values_are_clamped() {
        let mut config = VisualizerConfig {
            size: 9.0,
            bars: 9999,
            crt: -3.0,
            barrel: 8.0,
            particle_life: 0.0,
            particle_amount: 99.0,
            burst_life: 99.0,
            ..VisualizerConfig::default()
        };
        config.clamp();
        assert_eq!(config.size, SIZE_RANGE.1);
        assert_eq!(config.bars, BARS_RANGE.1);
        assert_eq!(config.crt, 0.0);
        assert_eq!(config.barrel, CRT_RANGE.1);
        assert_eq!(config.particle_life, PARTICLE_RANGE.0);
        assert_eq!(config.particle_amount, PARTICLE_AMOUNT_RANGE.1);
        assert_eq!(config.burst_life, PARTICLE_RANGE.1);
    }

    #[test]
    fn gains_are_reported_for_the_processor() {
        let config = VisualizerConfig {
            bass: 1.4,
            mid: 0.8,
            treble: 1.2,
            ..VisualizerConfig::default()
        };
        let gains = config.gains();
        for (actual, expected) in gains.iter().zip([1.4, 0.8, 1.2]) {
            assert!((actual - expected).abs() < 1e-6);
        }
    }

    #[test]
    fn palette_interpolates_between_stops() {
        let neon = Palette::Neon;
        assert_eq!(neon.color_at(0.0), neon.colors()[0]);
        assert_eq!(neon.color_at(1.0), neon.colors()[2]);
        let mid = neon.color_at(0.5);
        assert!(mid[0] > 0.4 && mid[0] < 0.9);
    }

    #[test]
    fn old_configs_without_new_fields_still_load() {
        let dir = std::env::temp_dir().join(format!("melo-visual-old-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("visualizer.json");
        std::fs::write(&path, r#"{"bars": 100, "palette": "ice"}"#).unwrap();

        let loaded = VisualizerConfig::load(&path);
        let defaults = VisualizerConfig::default();
        assert_eq!(loaded.bars, 100);
        assert_eq!(loaded.palette, Palette::Ice);
        assert!((loaded.bass - defaults.bass).abs() < 1e-6);
        assert!((loaded.barrel - defaults.barrel).abs() < 1e-6);
        assert!((loaded.particle_life - defaults.particle_life).abs() < 1e-6);
        assert_eq!(loaded.burst, loaded.particles);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn old_particles_toggle_migrates_to_burst() {
        let dir = std::env::temp_dir().join(format!("melo-visual-burst-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("visualizer.json");
        std::fs::write(&path, r#"{"particles": false}"#).unwrap();

        let loaded = VisualizerConfig::load(&path);
        assert!(!loaded.particles);
        assert!(!loaded.burst, "老配置关闭粒子时不应突然出现鼓点爆发");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn config_roundtrips_through_disk() {
        let dir = std::env::temp_dir().join(format!("melo-visual-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("visualizer.json");

        let config = VisualizerConfig {
            bars: 120,
            palette: Palette::Sunset,
            crt: 0.8,
            barrel: 0.2,
            burst: false,
            burst_speed: 2.0,
            burst_life: 0.5,
            particle_amount: 1.8,
            ..VisualizerConfig::default()
        };
        config.save(&path).expect("save");

        let loaded = VisualizerConfig::load(&path);
        assert_eq!(loaded.bars, 120);
        assert_eq!(loaded.palette, Palette::Sunset);
        assert!((loaded.crt - 0.8).abs() < 1e-6);
        assert!((loaded.barrel - 0.2).abs() < 1e-6);
        assert!(!loaded.burst);
        assert!((loaded.burst_speed - 2.0).abs() < 1e-6);
        assert!((loaded.burst_life - 0.5).abs() < 1e-6);
        assert!((loaded.particle_amount - 1.8).abs() < 1e-6);

        let _ = std::fs::remove_dir_all(&dir);
    }
}

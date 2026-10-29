//! 环形频谱场景（参考 VisualSpectrum）。
//!
//! 与 DSP 分开：`SpectrumProcessor` 只负责把采样变成 72 段平滑电平，
//! 这里负责把它铺成环形条、补上余晖拖尾和鼓点粒子。
//!
//! 参数由 [`VisualizerConfig`] 控制：条数、自转、余晖、粒子灵敏度、配色等。

use std::f64::consts::TAU;

use crate::visualizer::{VisualizerConfig, HEIGHT_RANGE, INTENSITY_RANGE};

const MAX_PARTICLES: usize = 700;
/// 每帧最多尝试生成的持续粒子数。
const MAX_SPAWN_PER_TICK: usize = 24;
/// 一次鼓点爆发最多多少粒子。
const MAX_BURST: usize = 140;

/// 显示电平的跟随速度（指数平滑，帧率无关）。
/// 参考 VisualSpectrum：上升 τ≈45ms、下降 τ≈154ms。
const ATTACK_RATE: f64 = 22.0;
const RELEASE_RATE: f64 = 6.5;

#[derive(Debug, Clone, Copy)]
pub struct Particle {
    pub angle: f32,
    pub radius: f32,
    pub speed: f32,
    pub life: f32,
    /// 设计寿命：到达后粒子已经淡出，可以从场景里移除。
    pub ttl: f32,
    pub size: f32,
    /// 在色标上的位置（0~1），绘制时用来归并颜色桶。
    pub color_t: f32,
}

impl Particle {
    /// 0 → 刚生成，1 → 完全消失。
    pub fn progress(&self) -> f32 {
        if self.ttl <= 0.0 {
            1.0
        } else {
            (self.life / self.ttl).clamp(0.0, 1.0)
        }
    }
}

/// 粒子裁剪用的可视区域（像素）：粒子中心飞出这块矩形才移除。
///
/// 画布现在铺满整个窗口，不再有圆形遮罩，所以粒子会一直漂到场外才消失。
#[derive(Debug, Clone, Copy, Default)]
pub struct ViewBounds {
    pub width: f32,
    pub height: f32,
    /// 环心在窗口中的位置。
    pub center: [f32; 2],
    /// 环的外径（像素）；粒子半径以它为 1.0。
    pub radius_outer: f32,
}

impl ViewBounds {
    fn ready(&self) -> bool {
        self.width > 0.0 && self.height > 0.0 && self.radius_outer > 0.0
    }
}

pub struct RingScene {
    /// 平滑后的 0~1 电平；长度跟随配置里的条数。
    pub shown: Vec<f64>,
    /// 拖尾用的滞后电平。
    pub trail: Vec<f64>,
    pub particles: Vec<Particle>,
    /// 自转角（弧度）。
    pub rotation: f32,
    /// 平滑后的低频能量（0~1），用于封面光晕等界面联动。
    pub bass: f32,
    /// 上一帧的秒数，后处理余晖按它计算衰减。
    pub last_dt: f32,
    /// 已检测到的鼓点次数（调试 / 测试）。
    pub beats: u64,
    /// 当前画布信息；由播放页每帧写入，用于把粒子裁到屏幕边缘。
    view: ViewBounds,
    /// 重采样后的目标电平（复用缓冲，避免每帧分配）。
    target: Vec<f64>,
    bass_avg: f64,
    since_beat: f32,
    rng: Rng,
}

impl RingScene {
    pub fn new() -> Self {
        Self {
            shown: vec![0.0; VisualizerConfig::default().bars as usize],
            trail: vec![0.0; VisualizerConfig::default().bars as usize],
            particles: Vec::new(),
            rotation: 0.0,
            bass: 0.0,
            last_dt: 1.0 / 60.0,
            beats: 0,
            view: ViewBounds::default(),
            target: Vec::new(),
            bass_avg: 0.0,
            since_beat: 99.0,
            rng: Rng::new(0x9E37_79B9_7F4A_7C15),
        }
    }

    /// 切歌时清空，避免上一首的余晖拖到新曲目上。
    pub fn reset(&mut self) {
        self.shown.iter_mut().for_each(|value| *value = 0.0);
        self.trail.iter_mut().for_each(|value| *value = 0.0);
        self.target.clear();
        self.particles.clear();
        self.bass_avg = 0.0;
        self.since_beat = 99.0;
        self.bass = 0.0;
        self.beats = 0;
    }

    /// 推进一帧。`dt` 单位秒，`bands` 是 DSP 输出的 0~1 电平。
    pub fn update(&mut self, dt: f32, bands: &[f64], config: &VisualizerConfig) {
        let dt = dt.clamp(0.001, 0.05);
        self.last_dt = dt;
        let count = config.bars.clamp(4, 512) as usize;
        if self.shown.len() != count {
            self.shown = vec![0.0; count];
            self.trail = vec![0.0; count];
        }

        resample_into(bands, count, &mut self.target);
        let gain = config.intensity.clamp(INTENSITY_RANGE.0, INTENSITY_RANGE.1) as f64;

        // 60fps 下也保持稳定：用时间常数而不是固定步长。
        let attack = 1.0 - (-ATTACK_RATE * dt as f64).exp();
        let release = 1.0 - (-RELEASE_RATE * dt as f64).exp();
        for (target, slot) in self.target.iter().zip(self.shown.iter_mut()) {
            let goal = (target * gain).clamp(0.0, 1.0);
            let k = if goal > *slot { attack } else { release };
            *slot += (goal - *slot) * k;
        }

        // 余晖：`afterglow` 越大，拖尾越慢消失。
        let follow = 1.0 - (-(2.0 + 22.0 * (1.0 - config.afterglow as f64)) * dt as f64).exp();
        for (shown, trail) in self.shown.iter().zip(self.trail.iter_mut()) {
            *trail += (*shown - *trail) * follow;
        }

        // 自转：0.4 rad/s ≈ 16 秒一圈。
        if config.rotate {
            let speed = 0.4 * config.rotate_speed.clamp(0.1, 4.0);
            self.rotation = (self.rotation + dt * speed) % TAU as f32;
        }

        // 低频能量：快起慢落，供封面光晕等界面元素跟随。
        let bass = self.bass_level();
        let bass_follow = if bass > self.bass as f64 {
            1.0 - (-18.0 * dt as f64).exp()
        } else {
            1.0 - (-3.5 * dt as f64).exp()
        };
        self.bass = (self.bass as f64 + (bass - self.bass as f64) * bass_follow) as f32;

        self.update_particles(dt);
        if config.particles {
            self.emit(dt, config);
        }
        if config.burst {
            self.detect_beat(dt, config, bass);
        }
    }

    /// 低频（前 8%）的平均电平，和 VisualSpectrum 的鼓点检测范围一致。
    fn bass_level(&self) -> f64 {
        let count = self.shown.len();
        if count == 0 {
            return 0.0;
        }
        let low = ((count as f64 * 0.08).round() as usize).max(1).min(count);
        self.shown[..low].iter().sum::<f64>() / low as f64
    }

    /// 写入当前画布的像素范围与环半径，粒子飞出这块矩形才移除。
    pub fn set_view(&mut self, view: ViewBounds) {
        self.view = view;
    }

    /// 平滑后的低频能量（0~1）。
    pub fn bass(&self) -> f32 {
        self.bass
    }

    /// 中心圆使用的低频能量：最外 4 根条的平均值。
    ///
    /// 必须和 `viz.rs` 里中心同心环的取值一致，封面才能和中心圆
    /// 同步起伏（`viz::center_scale`）。
    pub fn center_bass(&self) -> f32 {
        let count = self.shown.len().min(4);
        if count == 0 {
            return 0.0;
        }
        self.shown[..count].iter().sum::<f64>() as f32 / count as f32
    }

    /// 已检测到的鼓点次数。
    #[cfg(test)]
    pub fn beats(&self) -> u64 {
        self.beats
    }

    /// 从当前响亮的条尖持续飘出小粒子（参考 VisualSpectrum 的 emit）。
    fn emit(&mut self, _dt: f32, config: &VisualizerConfig) {
        let count = self.shown.len();
        if count == 0 || self.particles.len() >= MAX_PARTICLES {
            return;
        }
        let attempts = {
            let base = 4.0 + count as f32 * 0.06;
            let density = config.particle_amount.clamp(0.0, 2.5);
            if density <= 0.0 {
                return;
            }
            ((base * density).round() as usize).min(MAX_SPAWN_PER_TICK)
        };
        let speed_scale = config.particle_speed.clamp(0.2, 4.0);
        let life_scale = config.particle_life.clamp(0.2, 4.0);
        // 与 scene 一致：条高超过 100% 时底座收到圆心，粒子也从内圈往外飘。
        let height = config.height.clamp(HEIGHT_RANGE.0, HEIGHT_RANGE.1);
        let radius_base = (1.0 - height).max(0.0);

        for _ in 0..attempts {
            let index = self.rng.range(count);
            let value = self.shown[index];
            if self.rng.range_f32(0.0, 1.0) as f64 > value * value * 0.9 + 0.02 {
                continue;
            }
            if self.particles.len() >= MAX_PARTICLES {
                break;
            }
            let angle = index as f32 / count as f32 * TAU as f32 + self.rotation
                - std::f32::consts::FRAC_PI_2;
            self.particles.push(Particle {
                angle: angle + self.rng.range_f32(-0.025, 0.025),
                // 起点在内圈到条尖之间，与参考项目的 radius_base + max_len * value 等价。
                radius: radius_base + height * value as f32,
                speed: (0.06 + value as f32 * 0.30 + self.rng.range_f32(0.0, 0.09)) * speed_scale,
                life: 0.0,
                ttl: self.rng.range_f32(0.7, 1.8) * life_scale,
                size: self.rng.range_f32(0.8, 2.4),
                color_t: index as f32 / count as f32,
            });
        }
    }

    fn detect_beat(&mut self, dt: f32, config: &VisualizerConfig, bass: f64) {
        let sens = config.sensitivity.clamp(0.0, 1.0) as f64;
        let threshold = 1.55 + (1.04 - 1.55) * sens;
        let refractory = 0.36 + (0.07 - 0.36) * sens;
        self.since_beat += dt;

        if self.bass_avg > 0.0
            && bass > self.bass_avg * threshold
            && bass > 0.15
            && self.since_beat > refractory as f32
        {
            self.since_beat = 0.0;
            self.beats += 1;
            self.spawn_burst(config);
        }

        let follow = (dt as f64 * 1.5).min(1.0);
        if self.bass_avg == 0.0 {
            self.bass_avg = bass;
        } else {
            self.bass_avg += (bass - self.bass_avg) * follow;
        }
    }

    /// 鼓点爆发：像参考项目一样从环心附近向四周喷出。
    fn spawn_burst(&mut self, config: &VisualizerConfig) {
        let count = self.shown.len().max(1);
        let sens = config.sensitivity.clamp(0.0, 1.0);
        let total = (28.0 + 68.0 * sens).round() as usize;
        let spawn = total
            .min(MAX_BURST)
            .min(MAX_PARTICLES.saturating_sub(self.particles.len()));
        let speed_scale = config.burst_speed.clamp(0.2, 4.0);
        let life_scale = config.burst_life.clamp(0.2, 4.0);

        for _ in 0..spawn {
            let ttl = self.rng.range_f32(0.6, 1.5) * life_scale;
            // 归一化到“环半径”的速度：参考项目是 max_radius / ttl * 1.15。
            let speed = (1.5 / ttl.max(0.1)) * 1.15 * speed_scale;
            let index = self.rng.range(count);
            self.particles.push(Particle {
                angle: self.rng.range_f32(0.0, TAU as f32),
                radius: self.rng.range_f32(0.0, 0.03),
                speed,
                life: 0.0,
                ttl,
                size: self.rng.range_f32(1.4, 3.8),
                color_t: index as f32 / count as f32,
            });
        }
    }

    fn update_particles(&mut self, dt: f32) {
        for particle in &mut self.particles {
            particle.life += dt;
            // 速度单位是「每秒多少个环半径」，与画布大小无关。
            particle.radius += particle.speed * dt;
            particle.angle += dt * 0.15;
        }

        // 粒子按寿命淡出（`progress` 控制 alpha），到期就移除；
        // 如果它提前飞出窗口，也直接裁掉——屏幕外没有继续模拟的必要。
        // 留 12px 余量，避免在屏幕里最后一刻看到硬切。
        const MARGIN: f32 = 12.0;
        let view = self.view;
        self.particles.retain(|particle| {
            if particle.life >= particle.ttl {
                return false;
            }
            if !view.ready() {
                return true;
            }
            let (sin, cos) = particle.angle.sin_cos();
            let x = view.center[0] + cos * particle.radius * view.radius_outer;
            let y = view.center[1] + sin * particle.radius * view.radius_outer;
            x > -MARGIN && x < view.width + MARGIN && y > -MARGIN && y < view.height + MARGIN
        });
    }
}

impl Default for RingScene {
    fn default() -> Self {
        Self::new()
    }
}

/// 把任意长度的频段重采样到 `count` 根（线性插值）。
#[cfg(test)]
pub fn resample(bands: &[f64], count: usize) -> Vec<f64> {
    let mut out = Vec::with_capacity(count);
    resample_into(bands, count, &mut out);
    out
}

/// 同 [`resample`]，结果写进复用缓冲。
pub fn resample_into(bands: &[f64], count: usize, out: &mut Vec<f64>) {
    out.clear();
    out.resize(count, 0.0);
    if bands.is_empty() || count == 0 {
        return;
    }
    if bands.len() == count {
        out.copy_from_slice(bands);
        return;
    }
    if bands.len() == 1 {
        out.fill(bands[0]);
        return;
    }
    if count == 1 {
        out[0] = bands[bands.len() - 1];
        return;
    }

    let n = bands.len();
    for (i, slot) in out.iter_mut().enumerate() {
        let position = i as f64 / (count - 1) as f64 * (n - 1) as f64;
        let lo = position.floor() as usize;
        let hi = (lo + 1).min(n - 1);
        let k = position - lo as f64;
        *slot = bands[lo] + (bands[hi] - bands[lo]) * k;
    }
}

/// 很小的 xorshift，避免为了粒子引入 rand。
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0x9E37_79B9_7F4A_7C15
        } else {
            seed
        })
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn range(&mut self, max: usize) -> usize {
        if max == 0 {
            0
        } else {
            (self.next() % max as u64) as usize
        }
    }

    fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.next() >> 11) as f32 / (1u64 << 53) as f32;
        lo + (hi - lo) * unit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn particles_emit_only_when_enabled() {
        let loud = [0.9; 72];

        let config = VisualizerConfig::default();
        let mut scene = RingScene::new();
        for _ in 0..40 {
            scene.update(0.016, &loud, &config);
        }
        assert!(!scene.particles.is_empty(), "开启粒子时应当有持续发射");

        let config = VisualizerConfig {
            particles: false,
            burst: false,
            ..VisualizerConfig::default()
        };
        let mut scene = RingScene::new();
        for _ in 0..40 {
            scene.update(0.016, &loud, &config);
        }
        assert!(scene.particles.is_empty(), "关闭持续粒子后不应再生成");
    }

    #[test]
    fn burst_toggle_controls_drum_detection() {
        let run = |burst: bool| {
            let config = VisualizerConfig {
                burst,
                sensitivity: 0.8,
                ..VisualizerConfig::default()
            };
            let mut scene = RingScene::new();
            // 先安静一阵让低频参考线落低，再来一记骤升。
            for _ in 0..10 {
                scene.update(0.05, &[0.02; 72], &config);
            }
            for _ in 0..8 {
                scene.update(0.05, &[1.0; 72], &config);
            }
            scene.beats()
        };

        assert_eq!(run(false), 0, "关闭鼓点爆发后不应检测鼓点");
        assert!(run(true) > 0, "开启后低频骤升应当触发鼓点");
    }

    #[test]
    fn continuous_particles_can_be_off_while_burst_still_works() {
        let config = VisualizerConfig {
            particles: false,
            burst: true,
            sensitivity: 0.9,
            ..VisualizerConfig::default()
        };
        let mut scene = RingScene::new();
        for _ in 0..10 {
            scene.update(0.05, &[0.02; 72], &config);
        }
        for _ in 0..8 {
            scene.update(0.05, &[1.0; 72], &config);
        }
        assert!(scene.beats() > 0);
        assert!(!scene.particles.is_empty(), "鼓点爆发应当仍然喷出粒子");
    }

    #[test]
    fn burst_life_scales_particle_ttl() {
        let run = |life: f32| {
            let config = VisualizerConfig {
                particles: false,
                burst: true,
                burst_life: life,
                sensitivity: 0.9,
                ..VisualizerConfig::default()
            };
            let mut scene = RingScene::new();
            for _ in 0..10 {
                scene.update(0.05, &[0.02; 72], &config);
            }
            for _ in 0..8 {
                scene.update(0.05, &[1.0; 72], &config);
            }
            scene
                .particles
                .iter()
                .map(|particle| particle.ttl)
                .fold(0.0, f32::max)
        };

        assert!(run(2.0) > run(0.5), "爆发寿命倍数应当影响粒子存活时间");
    }

    #[test]
    fn particle_density_scales_continuous_emission() {
        let run = |amount: f32| {
            let config = VisualizerConfig {
                particles: true,
                burst: false,
                particle_amount: amount,
                ..VisualizerConfig::default()
            };
            let mut scene = RingScene::new();
            for _ in 0..30 {
                scene.update(0.04, &[0.9; 72], &config);
            }
            scene.particles.len()
        };

        assert_eq!(run(0.0), 0, "密度为 0 时不应发射音条粒子");
        assert!(run(2.0) > run(0.5), "密度越大持续粒子越多");
    }

    #[test]
    fn bass_energy_follows_low_bands() {
        let config = VisualizerConfig {
            particles: false,
            burst: false,
            ..VisualizerConfig::default()
        };
        let mut scene = RingScene::new();
        for _ in 0..20 {
            scene.update(0.04, &[0.9; 72], &config);
        }
        assert!(scene.bass() > 0.5, "低频能量应当跟随响亮的低频段");

        for _ in 0..60 {
            scene.update(0.04, &[0.0; 72], &config);
        }
        assert!(scene.bass() < 0.1, "静音后低频能量应当回落");
    }

    #[test]
    fn center_bass_averages_the_outer_four_bars() {
        let mut scene = RingScene::new();
        scene.shown = vec![0.0; 72];
        scene.shown[..4].copy_from_slice(&[0.8, 0.4, 0.0, 0.0]);
        assert!((scene.center_bass() - 0.3).abs() < 1e-6);

        // 后面的条不参与：中心圆只看最外 4 根。
        scene.shown[10] = 1.0;
        assert!((scene.center_bass() - 0.3).abs() < 1e-6);

        scene.shown.clear();
        assert_eq!(scene.center_bass(), 0.0);
    }

    #[test]
    fn particles_are_removed_early_when_they_leave_the_canvas() {
        let mut scene = RingScene::new();
        scene.set_view(ViewBounds {
            width: 800.0,
            height: 600.0,
            center: [400.0, 300.0],
            radius_outer: 200.0,
        });
        // 半径 3.0 = 600px，早已越过右边界；寿命还没到也应当移除。
        scene.particles = vec![
            Particle {
                angle: 0.0,
                radius: 1.0,
                speed: 0.0,
                life: 0.1,
                ttl: 5.0,
                size: 1.5,
                color_t: 0.0,
            },
            Particle {
                angle: 0.0,
                radius: 3.0,
                speed: 0.0,
                life: 0.1,
                ttl: 5.0,
                size: 1.5,
                color_t: 0.0,
            },
        ];

        scene.update_particles(0.016);

        assert_eq!(scene.particles.len(), 1, "只有飞出画布的粒子被移除");
        assert!(scene.particles[0].radius < 2.0);
    }

    #[test]
    fn particles_still_expire_after_their_lifetime() {
        let mut scene = RingScene::new();
        scene.set_view(ViewBounds {
            width: 800.0,
            height: 600.0,
            center: [400.0, 300.0],
            radius_outer: 200.0,
        });
        // 还在画布内，但已经超过寿命：移除（此时 alpha 也已经淡到 0）。
        scene.particles.push(Particle {
            angle: 0.0,
            radius: 1.0,
            speed: 0.0,
            life: 0.6,
            ttl: 0.5,
            size: 1.5,
            color_t: 0.0,
        });

        scene.update_particles(0.016);
        assert!(scene.particles.is_empty());
    }

    #[test]
    fn particles_fall_back_to_ttl_without_a_view() {
        let mut scene = RingScene::new();
        scene.particles.push(Particle {
            angle: 0.0,
            radius: 0.5,
            speed: 0.0,
            life: 2.0,
            ttl: 0.5,
            size: 1.0,
            color_t: 0.0,
        });
        scene.update_particles(0.016);
        assert!(scene.particles.is_empty(), "没有画布信息时按 ttl 兜底");
    }

    #[test]
    fn resample_matches_length_and_endpoints() {
        let bands = vec![0.0, 0.5, 1.0];
        let out = resample(&bands, 5);
        assert_eq!(out.len(), 5);
        assert!((out[0] - 0.0).abs() < 1e-9);
        assert!((out[4] - 1.0).abs() < 1e-9);
        assert!((out[2] - 0.5).abs() < 1e-9);
    }

    #[test]
    fn empty_input_decays_instead_of_panicking() {
        let config = VisualizerConfig::default();
        let mut scene = RingScene::new();
        scene.update(0.04, &[], &config);
        assert_eq!(scene.shown.len(), config.bars as usize);
        assert!(scene.shown.iter().all(|value| *value == 0.0));
    }

    #[test]
    fn rotation_advances_with_time() {
        let config = VisualizerConfig::default();
        let mut scene = RingScene::new();
        scene.update(0.5, &[0.5; 8], &config);
        assert!(scene.rotation > 0.0);
    }

    #[test]
    fn levels_stay_in_unit_range() {
        let config = VisualizerConfig {
            intensity: 2.0,
            ..VisualizerConfig::default()
        };
        let mut scene = RingScene::new();
        for _ in 0..40 {
            scene.update(0.04, &[1.2; 72], &config);
        }
        assert!(scene.shown.iter().all(|value| (0.0..=1.0).contains(value)));
    }

    #[test]
    fn bar_count_follows_config() {
        let config = VisualizerConfig {
            bars: 128,
            ..VisualizerConfig::default()
        };
        let mut scene = RingScene::new();
        scene.update(0.04, &[0.4; 64], &config);
        assert_eq!(scene.shown.len(), 128);
        assert_eq!(scene.trail.len(), 128);
    }
}

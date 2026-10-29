//! 浅色页面的装饰背景。
//!
//! 只铺在非「正在播放」页下面；播放页有自己的封面氛围底。
//!
//! 渐变在启动时烘焙成一张小尺寸贴图，之后每帧只画一个纹理 quad：
//! 比多层整窗渐变便宜得多，窗口缩放时由 GPU 线性插值拉伸，不会出现接缝。

use std::sync::OnceLock;

use iced::widget::image;
use iced::{Color, ContentFit, Length};

use crate::app::Element;
use crate::theme;

/// 贴图边长。渐变频率很低，放大后依然平滑；512² 只占 1 MB 内存。
const TEXTURE: u32 = 512;

pub fn layer() -> Element<'static> {
    image(handle().clone())
        .width(Length::Fill)
        .height(Length::Fill)
        .content_fit(ContentFit::Fill)
        .into()
}

fn handle() -> &'static image::Handle {
    static HANDLE: OnceLock<image::Handle> = OnceLock::new();

    HANDLE.get_or_init(|| {
        let mut pixels = Vec::with_capacity((TEXTURE * TEXTURE * 4) as usize);
        for y in 0..TEXTURE {
            let v = y as f32 / (TEXTURE - 1) as f32;
            for x in 0..TEXTURE {
                let u = x as f32 / (TEXTURE - 1) as f32;
                let [red, green, blue] = sample(u, v);
                pixels.extend_from_slice(&[to_byte(red), to_byte(green), to_byte(blue), 255]);
            }
        }
        image::Handle::from_rgba(TEXTURE, TEXTURE, pixels)
    })
}

/// 按归一化坐标 `(u, v)` 取一个背景像素：天空渐变 + 四角柔光。
fn sample(u: f32, v: f32) -> [f32; 3] {
    let top = [0.969, 0.980, 1.0];
    let middle = rgb(theme::BG);
    let bottom = [0.925, 0.933, 0.965];

    // 天空：上冷白 → 页面底色 → 下淡蓝灰。
    let mut color = if v <= 0.55 {
        mix(top, middle, v / 0.55)
    } else {
        mix(middle, bottom, (v - 0.55) / 0.45)
    };

    // 左上冷光：u + v 越大越淡。
    color = over(
        color,
        rgb(theme::ACCENT),
        ramp(u + v - 0.5, &[(0.0, 0.14), (0.48, 0.035), (1.0, 0.0)]),
    );
    // 右上紫光。
    color = over(
        color,
        rgb(theme::VIOLET),
        ramp(1.0 - u + v - 0.5, &[(0.0, 0.07), (0.42, 0.02), (1.0, 0.0)]),
    );
    // 右下暖光。
    color = over(
        color,
        rgb(theme::LIKE),
        ramp(1.5 - u - v, &[(0.0, 0.06), (0.45, 0.015), (1.0, 0.0)]),
    );
    // 左下一点青蓝，补全四角。
    color = over(
        color,
        rgb(theme::CYAN),
        ramp(u + 1.0 - v - 0.5, &[(0.0, 0.05), (0.4, 0.012), (1.0, 0.0)]),
    );

    color
}

fn ramp(value: f32, stops: &[(f32, f32)]) -> f32 {
    let value = value.clamp(0.0, 1.0);
    if value <= stops[0].0 {
        return stops[0].1;
    }

    for pair in stops.windows(2) {
        let (from_t, from_alpha) = pair[0];
        let (to_t, to_alpha) = pair[1];
        if value <= to_t {
            let t = ((value - from_t) / (to_t - from_t)).clamp(0.0, 1.0);
            // 和 iced 的渐变着色器一样用 smoothstep，避免出现折线。
            let t = t * t * (3.0 - 2.0 * t);
            return from_alpha + (to_alpha - from_alpha) * t;
        }
    }

    stops[stops.len() - 1].1
}

fn mix(from: [f32; 3], to: [f32; 3], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        from[0] + (to[0] - from[0]) * t,
        from[1] + (to[1] - from[1]) * t,
        from[2] + (to[2] - from[2]) * t,
    ]
}

/// 把半透明颜色叠在底色上。
fn over(base: [f32; 3], color: [f32; 3], alpha: f32) -> [f32; 3] {
    mix(base, color, alpha.clamp(0.0, 1.0))
}

fn rgb(color: Color) -> [f32; 3] {
    [color.r, color.g, color.b]
}

fn to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ramp_clamps_and_hits_both_ends() {
        let stops = [(0.0, 0.14), (0.5, 0.03), (1.0, 0.0)];

        assert!((ramp(-1.0, &stops) - 0.14).abs() < 1e-6);
        assert!((ramp(0.0, &stops) - 0.14).abs() < 1e-6);
        assert!((ramp(1.0, &stops) - 0.0).abs() < 1e-6);
        assert!((ramp(2.0, &stops) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn ramp_is_monotonic_between_stops() {
        let stops = [(0.0, 0.14), (0.48, 0.035), (1.0, 0.0)];
        let mut previous = ramp(0.0, &stops);
        for step in 1..=100 {
            let value = ramp(step as f32 / 100.0, &stops);
            assert!(value <= previous + 1e-6, "step {step}");
            previous = value;
        }
    }

    #[test]
    fn corners_get_their_own_tint() {
        let top_left = sample(0.0, 0.0);
        let top_right = sample(1.0, 0.0);
        let bottom_left = sample(0.0, 1.0);
        let bottom_right = sample(1.0, 1.0);
        let center = sample(0.5, 0.5);

        // 左上应该比中心更蓝（红通道更低、蓝通道更高）。
        assert!(top_left[0] < center[0]);
        assert!(top_left[2] >= center[2]);
        // 右下应该比中心更暖（红通道更高）。
        assert!(bottom_right[0] > center[0]);
        // 四角彼此不同色。
        assert_ne!(to_byte(top_right[0]), to_byte(bottom_left[0]));
    }
}

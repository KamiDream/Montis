//! 头像后处理：把网易云返回的方形封面裁成圆形，并保留透明通道。
//!
//! iced 的 `image` 组件不支持圆角裁剪；与其在布局里叠一层遮罩，
//! 不如在下载后直接生成一张圆形 PNG，渲染成本更低，观感也更干净。

use image::{Rgba, RgbaImage};

/// 解码图片 → 居中裁方 → 缩放 → 画圆形 alpha 遮罩 → 编码为 PNG。
///
/// 返回 `None` 表示图片无法解码或编码失败，调用方应退回昵称首字头像。
pub fn round(bytes: &[u8], size: u32) -> Option<Vec<u8>> {
    rounded(bytes, size, 0.5)
}

/// 与 [`round`] 相同，但支持指定圆角比例（0.0 ~ 0.5）。
/// 封面用 0.12 左右的小圆角，头像用 0.5 得到正圆。
pub fn rounded(bytes: &[u8], size: u32, radius_ratio: f32) -> Option<Vec<u8>> {
    let source = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (width, height) = source.dimensions();
    if width == 0 || height == 0 || size == 0 {
        return None;
    }

    let side = width.min(height);
    let offset_x = (width - side) / 2;
    let offset_y = (height - side) / 2;

    let mut output = RgbaImage::new(size, size);
    let extent = size as f32;
    let radius = (extent * radius_ratio).clamp(0.0, extent / 2.0);

    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            // 到圆角矩形“内核”的距离：四个角之外才算透明。
            let anchor_x = px.clamp(radius, extent - radius);
            let anchor_y = py.clamp(radius, extent - radius);
            let distance = ((px - anchor_x).powi(2) + (py - anchor_y).powi(2)).sqrt();

            if distance > radius + 1.0 {
                output.put_pixel(x, y, Rgba([0, 0, 0, 0]));
                continue;
            }

            // 最近邻采样对缩略图足够，且不引入额外依赖。
            let source_x = offset_x + (x as u64 * side as u64 / size as u64) as u32;
            let source_y = offset_y + (y as u64 * side as u64 / size as u64) as u32;
            let pixel = source.get_pixel(source_x.min(width - 1), source_y.min(height - 1));

            // 边缘 1px 做抗锯齿。
            let edge_alpha = if distance > radius {
                ((radius + 1.0 - distance) * 255.0).clamp(0.0, 255.0) as u16
            } else {
                255
            };
            let alpha = (u16::from(pixel[3]) * edge_alpha / 255) as u8;

            output.put_pixel(x, y, Rgba([pixel[0], pixel[1], pixel[2], alpha]));
        }
    }

    let mut buffer = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(output)
        .write_to(&mut buffer, image::ImageOutputFormat::Png)
        .ok()?;
    Some(buffer.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tolerates_garbage() {
        assert!(round(b"not an image", 64).is_none());
    }

    #[test]
    fn rounded_corners_are_transparent() {
        let source = image::RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        source
            .write_to(&mut bytes, image::ImageOutputFormat::Png)
            .expect("encode");

        let output = rounded(bytes.get_ref(), 16, 0.3).expect("rounded");
        let decoded = image::load_from_memory(&output).expect("decode").to_rgba8();

        assert_eq!(
            decoded.get_pixel(0, 0)[3],
            0,
            "corner should be transparent"
        );
        assert_eq!(decoded.get_pixel(8, 8)[3], 255, "center should be opaque");
    }
}

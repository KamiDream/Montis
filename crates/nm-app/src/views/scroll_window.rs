//! 长列表虚拟化：只渲染视口内的行，用上下占位空间撑住总高度。
//!
//! 滚动内容的总高度必须严格等于 `总行数 × 行高`，否则滚动条会随滚动来回跳。

/// 计算虚拟化窗口：`(首个可见行, 结束行, 上方占位高度, 下方占位高度)`。
///
/// `view_height` 是滚动容器的可视高度，`row_height` 是每行的固定高度，
/// `overscan` 是视口上下额外保留的行数（减少快速滚动时的空白）。
pub(crate) fn visible_window(
    total: usize,
    offset: f32,
    view_height: f32,
    row_height: f32,
    overscan: usize,
) -> (usize, usize, f32, f32) {
    if total == 0 || row_height <= 0.0 {
        return (0, 0, 0.0, 0.0);
    }

    let content_height = total as f32 * row_height;
    let offset = offset.clamp(0.0, (content_height - view_height).max(0.0));

    let first = ((offset / row_height).floor() as usize).saturating_sub(overscan);
    let visible = (view_height.max(row_height) / row_height).ceil() as usize + overscan * 2;
    let last = (first + visible).min(total);

    (
        first,
        last,
        first as f32 * row_height,
        (total - last) as f32 * row_height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_list_renders_everything() {
        assert_eq!(visible_window(10, 0.0, 600.0, 50.0, 4), (0, 10, 0.0, 0.0));
    }

    #[test]
    fn scrolled_window_skips_off_screen_rows() {
        let (first, last, top, bottom) = visible_window(1000, 50.0 * 500.0, 500.0, 50.0, 4);

        assert!(first > 0 && first < 500, "first = {first}");
        assert!(last > first && last < 1000, "last = {last}");
        // 占位高度 + 渲染行高度恰好等于完整列表高度，滚动条才不会跳动。
        assert_eq!(top + (last - first) as f32 * 50.0 + bottom, 1000.0 * 50.0);
    }

    #[test]
    fn offset_past_the_end_is_clamped() {
        let (first, last, _, bottom) = visible_window(100, 1_000_000.0, 400.0, 50.0, 4);

        assert!(first < 100);
        assert_eq!(last, 100);
        assert_eq!(bottom, 0.0);
    }

    #[test]
    fn negative_offset_starts_at_the_top() {
        let (first, last, top, _) = visible_window(100, -50.0, 400.0, 50.0, 4);

        assert_eq!(first, 0);
        assert!(last > 0 && last <= 100);
        assert_eq!(top, 0.0);
    }

    #[test]
    fn empty_list_has_an_empty_window() {
        assert_eq!(visible_window(0, 0.0, 500.0, 50.0, 4), (0, 0, 0.0, 0.0));
        assert_eq!(visible_window(10, 0.0, 500.0, 0.0, 4), (0, 0, 0.0, 0.0));
    }

    #[test]
    fn very_short_viewport_still_renders_at_least_one_row() {
        let (first, last, ..) = visible_window(100, 0.0, 10.0, 50.0, 0);
        assert_eq!((first, last), (0, 1));
    }
}

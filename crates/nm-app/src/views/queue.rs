//! 播放队列面板：查看当前队列、跳播、移除与清空。

use iced::widget::{
    column, container, horizontal_space, image, mouse_area, row, scrollable, text, vertical_space,
};
use iced::{border, mouse, Alignment, Background, ContentFit, Length, Padding};

use nm_api::Song;

use crate::app::{App, Element, Message, RepeatMode, ScrollArea};
use crate::theme;
use crate::widgets;

/// 单行高度：36px 封面 + 上下各 7px 内边距，另留 2px 行间距。
pub(crate) const ROW_HEIGHT: f32 = 52.0;
/// 视口上下额外保留的行数，减少快速滚动时的空白。
const OVERSCAN: usize = 8;

pub fn panel(app: &App) -> Element<'_> {
    // 头部固定，仅列表滚动；滚动偏移用于虚拟化只渲染可见行。
    let list = scrollable(
        container(queue_list(app))
            .width(Length::Fill)
            // 给右侧滚动条留出空间，避免压住时长和文字。
            .padding(Padding {
                top: 0.0,
                right: 14.0,
                bottom: 14.0,
                left: 0.0,
            }),
    )
    .id(list_id())
    .height(Length::Fill)
    .on_scroll(|viewport| Message::QueueScrolled(viewport.absolute_offset().y))
    .style(theme::scrollable_auto(
        app.scroll_opacity(ScrollArea::Queue),
    ));

    let body = column![
        widgets::gradient_rule(2.0),
        header(app),
        widgets::separator(),
        list
    ]
    .spacing(12)
    .width(Length::Fill)
    .height(Length::Fill);

    let card = container(body)
        .width(Length::Fixed(380.0))
        .height(Length::Fill)
        .padding(Padding::from(16))
        .style(theme::card);

    let backdrop = mouse_area(
        container(row![])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::veil(0.18)),
    )
    .on_press(Message::ToggleQueue);

    iced::widget::stack![
        backdrop,
        container(row![horizontal_space(), iced::widget::opaque(card)])
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding::from(18)),
    ]
    .into()
}

fn header(app: &App) -> Element<'_> {
    let total = app.player.queue.len();
    let summary = if total == 0 {
        "队列为空".to_string()
    } else {
        format!(
            "{} 首 · 第 {} 首",
            total,
            app.player.index.min(total - 1) + 1
        )
    };

    let clear: Element<'_> = if total > 0 {
        mouse_area(text("清空队列").size(11).color(theme::DIM))
            .on_press(Message::QueueClear)
            .interaction(mouse::Interaction::Pointer)
            .into()
    } else {
        row![].into()
    };

    column![
        row![
            column![
                row![
                    widgets::dot(theme::ACCENT, 5.0),
                    text("QUEUE").size(9).color(theme::DIM2),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                text("播放队列").size(17).color(theme::FG),
                text(summary).size(10).color(theme::DIM),
            ]
            .spacing(3),
            horizontal_space(),
            mouse_area(text("关闭").size(11).color(theme::DIM))
                .on_press(Message::ToggleQueue)
                .interaction(mouse::Interaction::Pointer),
        ]
        .align_y(Alignment::Center),
        row![mode_pills(app), horizontal_space(), clear,].align_y(Alignment::Center),
    ]
    .spacing(10)
    .into()
}

fn mode_pills(app: &App) -> Element<'_> {
    let mode = app.player.mode;

    row![
        mode_pill(
            widgets::ICON_SHUFFLE,
            "随机",
            mode.shuffle,
            Message::ToggleShuffle,
        ),
        mode_pill(
            widgets::repeat_icon(mode.repeat),
            mode.repeat.label(),
            mode.repeat != RepeatMode::Off,
            Message::CycleRepeat,
        ),
    ]
    .spacing(5)
    .align_y(Alignment::Center)
    .into()
}

fn mode_pill(
    icon: &'static [u8],
    label: &'static str,
    active: bool,
    message: Message,
) -> Element<'static> {
    let color = if active { theme::ACCENT } else { theme::DIM };

    mouse_area(
        container(
            row![
                widgets::svg_icon(icon, 11.0, color),
                text(label).size(10).color(color),
            ]
            .spacing(5)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([3, 8]))
        .style(move |_| container::Style {
            background: Some(Background::Color(if active {
                iced::Color::from_rgb(0.910, 0.933, 1.0)
            } else {
                theme::TRANSPARENT
            })),
            border: border::rounded(999.0),
            ..container::Style::default()
        }),
    )
    .on_press(message)
    .interaction(mouse::Interaction::Pointer)
    .into()
}

fn queue_list(app: &App) -> Element<'_> {
    let total = app.player.queue.len();
    if total == 0 {
        return widgets::hint("播放队列还是空的。在歌曲行点 ＋ 添加到下一首。");
    }

    // 用窗口高度估算可视行数；真实滚动偏移由 on_scroll 持续回传。
    let view_height = app.viewport.height.max(ROW_HEIGHT);
    let (first, last, top, bottom) = visible_window(total, app.queue_scroll, view_height);

    // 行高固定，用占位空间补足屏幕外的行：无论渲染多少行，
    // 滚动内容的总高度都保持一致，滚动条不会因为虚拟化而跳动。
    let mut list = column![].width(Length::Fill);
    if top > 0.0 {
        list = list.push(vertical_space().height(top));
    }
    for (index, song) in app.player.queue.iter().enumerate().take(last).skip(first) {
        list = list.push(
            container(queue_row(app, index, song))
                .width(Length::Fill)
                .height(Length::Fixed(ROW_HEIGHT)),
        );
    }
    if bottom > 0.0 {
        list = list.push(vertical_space().height(bottom));
    }
    list.into()
}

/// 队列列表滚动容器的固定 ID：面板关闭后再打开时，用它恢复到上次的位置。
pub(crate) fn list_id() -> scrollable::Id {
    scrollable::Id::new("melo-queue-list")
}

/// 把滚动偏移限制在内容范围内，避免记住的位置超出新内容高度。
pub(crate) fn clamp_offset(total: usize, offset: f32, view_height: f32) -> f32 {
    let content_height = total as f32 * ROW_HEIGHT;
    let max_offset = (content_height - view_height).max(0.0);
    offset.clamp(0.0, max_offset)
}

/// 计算虚拟化窗口：`(首个可见行, 结束行, 上方占位高度, 下方占位高度)`。
pub(crate) fn visible_window(
    total: usize,
    offset: f32,
    view_height: f32,
) -> (usize, usize, f32, f32) {
    super::scroll_window::visible_window(total, offset, view_height, ROW_HEIGHT, OVERSCAN)
}

fn queue_row<'a>(app: &'a App, index: usize, song: &'a Song) -> Element<'a> {
    let active = index == app.player.index && app.player.current.is_some();

    let marker: Element<'a> = if active {
        widgets::equalizer_live(&app.player.spectrum, 3, 2.0, 14.0, theme::ACCENT)
    } else {
        text(format!("{:02}", index + 1))
            .size(10)
            .color(theme::DIM2)
            .into()
    };

    let title = column![
        text(clip(&song.name, 28))
            .size(12)
            .color(if active { theme::ACCENT } else { theme::FG }),
        text(clip(&song.artist_names(), 34))
            .size(9)
            .color(theme::DIM),
    ]
    .spacing(2)
    .width(Length::Fill);

    let main = row![
        container(marker)
            .width(22)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
        cover(app, &song.cover(96), 36.0),
        title,
        text(song.duration()).size(9).color(theme::DIM2),
    ]
    .spacing(9)
    .align_y(Alignment::Center)
    .padding(Padding::from([7, 6]));

    let play = mouse_area(main)
        .on_press(Message::QueuePlay(index))
        .interaction(mouse::Interaction::Pointer);

    let remove = mouse_area(
        container(text("×").size(14).color(theme::DIM2))
            .width(22)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
    )
    .on_press(Message::QueueRemove(index))
    .interaction(mouse::Interaction::Pointer);

    let like = widgets::like_button(
        app.is_liked(song.id),
        12.0,
        theme::DIM2,
        Message::ToggleLike(song.clone()),
    );

    container(
        row![container(play).width(Length::Fill), like, remove]
            .align_y(Alignment::Center)
            .padding(Padding::from([0, 2])),
    )
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(Background::Color(if active {
            iced::Color::from_rgb(0.914, 0.937, 1.0)
        } else {
            theme::TRANSPARENT
        })),
        border: border::rounded(8.0),
        ..container::Style::default()
    })
    .into()
}

fn cover<'a>(app: &'a App, url: &str, size: f32) -> Element<'a> {
    match app.covers.get(url) {
        Some(handle) => image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .into(),
        None => container(text("♪").size(size * 0.4).color(theme::DIM2))
            .width(size)
            .height(size)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(|_| container::Style {
                background: Some(Background::Color(theme::HOVER_BG)),
                border: border::rounded(6.0),
                ..container::Style::default()
            })
            .into(),
    }
}

fn clip(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let mut output: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        output.push('…');
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_queue_renders_everything() {
        let (first, last, top, bottom) = visible_window(10, 0.0, 600.0);
        assert_eq!((first, last), (0, 10));
        assert_eq!((top, bottom), (0.0, 0.0));
    }

    #[test]
    fn scrolled_window_skips_off_screen_rows() {
        let (first, last, top, bottom) = visible_window(1000, ROW_HEIGHT * 500.0, 500.0);

        assert!(first > 0 && first < 500, "first = {first}");
        assert!(last > first && last < 1000, "last = {last}");
        // 占位高度 + 渲染行高度恰好等于完整列表高度，滚动条才不会跳动。
        assert_eq!(
            top + (last - first) as f32 * ROW_HEIGHT + bottom,
            1000.0 * ROW_HEIGHT
        );
    }

    #[test]
    fn offset_past_the_end_is_clamped_to_the_last_page() {
        let (first, last, _, bottom) = visible_window(100, 1_000_000.0, 400.0);

        assert!(first < 100);
        assert_eq!(last, 100);
        assert_eq!(bottom, 0.0);
    }

    #[test]
    fn negative_offset_starts_at_the_top() {
        let (first, last, top, _) = visible_window(100, -50.0, 400.0);

        assert_eq!(first, 0);
        assert!(last > 0 && last <= 100);
        assert_eq!(top, 0.0);
    }

    #[test]
    fn empty_queue_has_an_empty_window() {
        assert_eq!(visible_window(0, 0.0, 500.0), (0, 0, 0.0, 0.0));
    }

    #[test]
    fn clamp_offset_keeps_positions_that_still_fit() {
        assert_eq!(clamp_offset(100, 500.0, 400.0), 500.0);
    }

    #[test]
    fn clamp_offset_pulls_back_when_content_shrinks() {
        // 队列只剩 10 首、视口 400px 时，最大偏移是 10 * 52 - 400 = 120。
        assert_eq!(clamp_offset(10, 900.0, 400.0), 10.0 * ROW_HEIGHT - 400.0);
    }

    #[test]
    fn clamp_offset_resets_when_content_is_shorter_than_view() {
        assert_eq!(clamp_offset(3, 260.0, 600.0), 0.0);
        assert_eq!(clamp_offset(0, 260.0, 600.0), 0.0);
    }
}

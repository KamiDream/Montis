use std::sync::OnceLock;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, Widget};
use iced::advanced::{overlay, renderer, Clipboard, Renderer as _, Shell};
use iced::widget::svg::{self, Handle};
use iced::widget::{button, container, image, mouse_area, row, text, vertical_space};
use iced::{
    border, mouse, Alignment, Background, Color, ContentFit, Event, Length, Padding, Rectangle,
    Shadow, Size, Vector,
};

use crate::app::{App, Element, Message, RepeatMode};
use crate::theme;
use nm_api::Song;

pub const LOGO: &[u8] = include_bytes!("../assets/logo.png");
pub const ICON_COOKIE: &[u8] = include_bytes!("../assets/cookie.svg");
pub const ICON_HOME: &[u8] = include_bytes!("../assets/home.svg");
pub const ICON_SEARCH: &[u8] = include_bytes!("../assets/search.svg");
pub const ICON_DAILY: &[u8] = include_bytes!("../assets/daily.svg");
pub const ICON_FM: &[u8] = include_bytes!("../assets/radio.svg");
pub const ICON_EXIT: &[u8] = include_bytes!("../assets/exit.svg");
pub const ICON_MINIMIZE: &[u8] = include_bytes!("../assets/minimize.svg");
pub const ICON_CLOSE: &[u8] = include_bytes!("../assets/close.svg");
pub const ICON_PLAY: &[u8] = include_bytes!("../assets/play.svg");
pub const ICON_PAUSE: &[u8] = include_bytes!("../assets/pause.svg");
pub const ICON_PREV: &[u8] = include_bytes!("../assets/prev.svg");
pub const ICON_NEXT: &[u8] = include_bytes!("../assets/next.svg");
pub const ICON_VOLUME: &[u8] = include_bytes!("../assets/volume.svg");
pub const ICON_VOLUME_MUTE: &[u8] = include_bytes!("../assets/volume_mute.svg");
pub const ICON_EXPAND: &[u8] = include_bytes!("../assets/expand.svg");
pub const ICON_BACK: &[u8] = include_bytes!("../assets/back.svg");
pub const ICON_SETTINGS: &[u8] = include_bytes!("../assets/settings.svg");
pub const ICON_INFO: &[u8] = include_bytes!("../assets/info.svg");
pub const ICON_QUEUE: &[u8] = include_bytes!("../assets/queue.svg");
pub const ICON_REFRESH: &[u8] = include_bytes!("../assets/refresh.svg");
pub const ICON_SEQUENCE: &[u8] = include_bytes!("../assets/sequence.svg");
pub const ICON_REPEAT: &[u8] = include_bytes!("../assets/repeat.svg");
pub const ICON_REPEAT_ONE: &[u8] = include_bytes!("../assets/repeat_one.svg");
pub const ICON_SHUFFLE: &[u8] = include_bytes!("../assets/shuffle.svg");
pub const ICON_HEART: &[u8] = include_bytes!("../assets/heart.svg");
pub const ICON_HEART_FILLED: &[u8] = include_bytes!("../assets/heart_filled.svg");
pub const ICON_DOWNLOAD: &[u8] = include_bytes!("../assets/download.svg");
pub const ICON_DOWNLOADED: &[u8] = include_bytes!("../assets/downloaded.svg");
pub const ICON_TRASH: &[u8] = include_bytes!("../assets/trash.svg");
pub const ICON_FOLDER: &[u8] = include_bytes!("../assets/folder.svg");

#[derive(Clone, Copy)]
pub enum PillKind {
    Ok,
    Warn,
    Muted,
    /// 红心收藏：浅粉底 + 红色文字。
    Like,
}

/// 循环模式对应的图标：顺序 / 列表循环 / 单曲循环。
pub fn repeat_icon(repeat: RepeatMode) -> &'static [u8] {
    match repeat {
        RepeatMode::Off => ICON_SEQUENCE,
        RepeatMode::All => ICON_REPEAT,
        RepeatMode::One => ICON_REPEAT_ONE,
    }
}

pub fn logo(size: f32) -> Element<'static> {
    // 源图是方形 PNG，直接显示会在浅色背景上露出黑边：预先裁成圆形再缓存。
    // `Handle::from_bytes` 每次都会生成新 ID，缓存起来避免每帧重新解码。
    static HANDLE: OnceLock<iced::widget::image::Handle> = OnceLock::new();
    let handle = HANDLE
        .get_or_init(|| {
            let bytes = crate::avatar::round(LOGO, 160).unwrap_or_else(|| LOGO.to_vec());
            iced::widget::image::Handle::from_bytes(bytes)
        })
        .clone();

    image(handle)
        .width(size)
        .height(size)
        .content_fit(ContentFit::Contain)
        .into()
}

pub fn svg_icon(icon: &'static [u8], size: f32, color: iced::Color) -> Element<'static> {
    iced::widget::svg(svg_handle(icon))
        .width(size)
        .height(size)
        .style(move |_, _| svg::Style { color: Some(color) })
        .into()
}

/// 红心收藏按钮：未收藏是描边，已收藏是实心红。
///
/// 用 `mouse_area` 而不是 `button`：它会嵌在整行/整条播放器的点击区里，
/// `mouse_area` 在按下时就会截获事件，不会顺带触发外层的播放或打开页面。
pub fn like_button(
    liked: bool,
    size: f32,
    inactive: iced::Color,
    message: Message,
) -> Element<'static> {
    let icon = if liked { ICON_HEART_FILLED } else { ICON_HEART };
    let color = if liked { theme::LIKE } else { inactive };

    mouse_area(
        container(svg_icon(icon, size, color))
            .padding(Padding::from(3.0))
            .center_x(Length::Fixed(size + 6.0))
            .center_y(Length::Fixed(size + 6.0)),
    )
    .on_press(message)
    .interaction(mouse::Interaction::Pointer)
    .into()
}

/// 下载状态按钮：未下载显示下载图标，已下载显示对勾，下载中显示三点动画。
///
/// `local` 为 true 表示在「本地音乐」页里：已下载的曲目改用垃圾桶图标，
/// 点击直接从本地删除；其它页面点击已下载的曲目只弹一条提示。
pub fn download_button<'a>(
    app: &crate::app::App,
    song: &Song,
    size: f32,
    local: bool,
) -> Element<'a> {
    // 音乐文件夹里的歌曲不是「下载」：不显示下载按钮，用同宽空位保持列对齐。
    if !song.local_path.is_empty() {
        return container(row![])
            .width(Length::Fixed(size + 6.0))
            .height(Length::Fixed(size + 6.0))
            .into();
    }

    if app.is_downloading(song.id) {
        return container(spinner(app.phase, 2.6, theme::ACCENT))
            .width(size + 6.0)
            .height(size + 6.0)
            .center_x(Length::Fixed(size + 6.0))
            .center_y(Length::Fixed(size + 6.0))
            .into();
    }

    let downloaded = app.is_downloaded(song.id);
    let (icon, color, message) = if downloaded {
        if local {
            (ICON_TRASH, theme::DIM, Message::RemoveDownload(song.id))
        } else {
            (
                ICON_DOWNLOADED,
                theme::ACCENT,
                Message::DownloadSong(song.clone()),
            )
        }
    } else {
        (
            ICON_DOWNLOAD,
            theme::DIM2,
            Message::DownloadSong(song.clone()),
        )
    };

    mouse_area(
        container(svg_icon(icon, size, color))
            .padding(Padding::from(3.0))
            .center_x(Length::Fixed(size + 6.0))
            .center_y(Length::Fixed(size + 6.0)),
    )
    .on_press(message)
    .interaction(mouse::Interaction::Pointer)
    .into()
}

/// 按图标数据缓存 `Handle`：`Handle::from_memory` 每次都会生成新 ID，
/// 直接放在 `view` 里会导致高频刷新（播放页 8ms 一帧）时每帧重新解析 SVG。
fn svg_handle(icon: &'static [u8]) -> Handle {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static CACHE: OnceLock<Mutex<HashMap<(usize, usize), Handle>>> = OnceLock::new();
    let key = (icon.as_ptr() as usize, icon.len());
    let mut cache = CACHE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    cache
        .entry(key)
        .or_insert_with(|| Handle::from_memory(icon))
        .clone()
}

/// 刷新按钮：淡蓝圆形底 + 刷新图标；加载中时换成三点动画。
pub fn refresh_button<'a>(busy: bool, phase: f32, message: Message) -> Element<'a> {
    if busy {
        container(spinner(phase, 2.4, theme::ACCENT))
            .width(24)
            .height(24)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(0.231, 0.447, 1.0, 0.09))),
                border: border::rounded(999.0)
                    .width(1.0)
                    .color(Color::from_rgba(0.231, 0.447, 1.0, 0.20)),
                ..container::Style::default()
            })
            .into()
    } else {
        button(svg_icon(ICON_REFRESH, 14.0, theme::ACCENT))
            .on_press(message)
            .padding(Padding::from([5, 5]))
            .style(theme::soft_icon)
            .into()
    }
}

/// 歌曲作者：每个作者分别可点击，进入各自的歌手页；没有可用歌手 ID 时退化成普通文本。
pub fn artist_link<'a>(song: &'a Song, max: usize, size: f32, color: Color) -> Element<'a> {
    let segments = artist_segments(song, max);
    let mut parts: Vec<Element<'a>> = Vec::with_capacity(segments.len() * 2);

    for (index, (name, target)) in segments.into_iter().enumerate() {
        if index > 0 {
            parts.push(text(" / ").size(size).color(color).into());
        }

        let label = text(name).size(size).color(color);
        parts.push(match target {
            Some(id) => mouse_area(label)
                .on_press(Message::OpenArtist(id))
                .interaction(mouse::Interaction::Pointer)
                .into(),
            None => label.into(),
        });
    }

    iced::widget::Row::with_children(parts)
        .align_y(Alignment::Center)
        .into()
}

/// 把「A / B / C」拆成单独的片段，每段带自己的歌手 ID，并按总字数预算截断。
fn artist_segments(song: &Song, max: usize) -> Vec<(String, Option<u64>)> {
    let artists: Vec<_> = song
        .ar
        .iter()
        .filter(|artist| !artist.name.is_empty())
        .collect();

    if artists.is_empty() {
        return vec![(clip_text(&song.artist_names(), max), None)];
    }

    let mut segments: Vec<(String, Option<u64>)> = Vec::with_capacity(artists.len());
    let mut remaining = max;

    for (index, artist) in artists.iter().enumerate() {
        // 分隔符「 / 」占 3 个字符；放不下就不加这个作者。
        let separator = if index > 0 { 3 } else { 0 };
        if remaining <= separator {
            if let Some((last, _)) = segments.last_mut() {
                if !last.ends_with('…') {
                    last.push('…');
                }
            }
            break;
        }
        remaining -= separator;

        let mut name = clip_text(&artist.name, remaining);
        let truncated = name != artist.name;
        if truncated && !name.ends_with('…') {
            name.push('…');
        }
        remaining = if truncated {
            0
        } else {
            remaining.saturating_sub(artist.name.chars().count())
        };

        segments.push((name, (artist.id != 0).then_some(artist.id)));
        if truncated {
            break;
        }
    }

    segments
}

/// 截断过长文本。
fn clip_text(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let mut output: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        output.push('…');
    }
    output
}

/// 悬浮提示胶囊：深色底 + 白字，`opacity` 控制整体淡入淡出。
pub fn toast<'a>(message: &'a str, opacity: f32) -> Element<'a> {
    let opacity = opacity.clamp(0.0, 1.0);
    container(
        row![
            container(row![])
                .width(6)
                .height(6)
                .style(move |_| container::Style {
                    background: Some(Background::Color(Color {
                        a: opacity,
                        ..theme::ACCENT
                    })),
                    border: border::rounded(3.0),
                    ..container::Style::default()
                }),
            text(message).size(12).color(Color {
                a: 0.96 * opacity,
                ..Color::WHITE
            }),
        ]
        .spacing(9)
        .align_y(Alignment::Center),
    )
    .padding(Padding::from([10, 18]))
    .style(move |_| container::Style {
        background: Some(Background::Color(Color {
            a: 0.92 * opacity,
            ..theme::NIGHT
        })),
        border: border::rounded(999.0).width(1.0).color(Color::from_rgba(
            1.0,
            1.0,
            1.0,
            0.14 * opacity,
        )),
        shadow: Shadow {
            color: Color::from_rgba(0.04, 0.08, 0.2, 0.18 * opacity),
            offset: Vector::new(0.0, 6.0),
            blur_radius: 18.0,
        },
        ..container::Style::default()
    })
    .into()
}

/// 悬浮在窗口右上角的窗口按钮（最小化 / 关闭）。
///
/// 顶栏已经移除，页面直接铺满窗口；只在最上面留一条 12px 的透明拖拽区，
/// 既能拖动窗口，又不会压住各页面自己的顶部控件。
/// `dark` 用于正在播放页（深色背景）切换图标颜色；`viz_settings` 为真时
/// 把可视化设置齿轮也收进来，三个按钮共用一枚半透明胶囊，不再散着漂浮。
pub fn window_controls(dark: bool, viz_settings: bool) -> Element<'static> {
    let color = if dark { theme::WHITE_70 } else { theme::DIM };
    let icon_style: fn(&iced::Theme, button::Status) -> button::Style =
        if dark { theme::icon_dark } else { theme::icon };
    let close_style: fn(&iced::Theme, button::Status) -> button::Style = if dark {
        theme::window_close_dark
    } else {
        theme::window_close
    };

    let mut buttons = row![].spacing(2).align_y(Alignment::Center);

    if viz_settings {
        buttons = buttons.push(
            button(svg_icon(ICON_SETTINGS, 15.0, color))
                .on_press(Message::ToggleSettings)
                .padding(Padding::from([7, 9]))
                .style(icon_style),
        );
        // 齿轮和窗口按钮之间一条淡淡的竖线，说明它们是一组。
        buttons = buttons.push(
            container(row![])
                .width(Length::Fixed(1.0))
                .height(Length::Fixed(14.0))
                .style(|_| container::Style {
                    background: Some(Background::Color(iced::Color::from_rgba(
                        1.0, 1.0, 1.0, 0.18,
                    ))),
                    ..container::Style::default()
                }),
        );
    }

    buttons = buttons
        .push(
            button(svg_icon(ICON_MINIMIZE, 14.0, color))
                .on_press(Message::Minimize)
                .padding(Padding::from([7, 9]))
                .style(icon_style),
        )
        .push(
            button(svg_icon(ICON_CLOSE, 14.0, color))
                .on_press(Message::Close)
                .padding(Padding::from([7, 9]))
                .style(close_style),
        );

    // 播放页的按钮压在明亮的频谱上，给一枚半透明胶囊让它们“有地方待着”，
    // 浅色页面保持原来的无背景样式。
    let controls: Element<'static> = if dark {
        container(buttons)
            .padding(Padding::from([3, 4]))
            .style(|_| container::Style {
                background: Some(Background::Color(iced::Color::from_rgba(
                    0.02, 0.03, 0.08, 0.38,
                ))),
                border: border::rounded(999.0)
                    .width(1.0)
                    .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.16)),
                shadow: Shadow {
                    color: iced::Color::from_rgba(0.0, 0.0, 0.0, 0.28),
                    offset: Vector::new(0.0, 4.0),
                    blur_radius: 14.0,
                },
                ..container::Style::default()
            })
            .into()
    } else {
        buttons.into()
    };

    let drag = mouse_area(
        container(row![])
            .width(Length::Fill)
            .height(Length::Fixed(12.0)),
    )
    .on_press(Message::DragWindow)
    .interaction(mouse::Interaction::Grab);

    iced::widget::stack![
        container(drag)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_y(Alignment::Start),
        container(controls)
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::Start)
            .padding(Padding::from([8, 10])),
    ]
    .into()
}

pub fn card<'a>(content: impl Into<Element<'a>>) -> Element<'a> {
    container(content)
        .padding(18)
        .width(Length::Fill)
        .style(theme::card)
        .into()
}

/// 用在品牌色卡片上的浅色胶囊：靠白色半透明与底图拉开层次。
pub fn brand_pill<'a>(label: impl Into<String>) -> Element<'a> {
    container(text(label.into()).size(11).color(iced::Color::WHITE))
        .padding(Padding::from([2, 9]))
        .style(|_| container::Style {
            background: Some(Background::Color(iced::Color::from_rgba(
                1.0, 1.0, 1.0, 0.16,
            ))),
            border: border::rounded(999.0)
                .width(1.0)
                .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.28)),
            text_color: Some(iced::Color::WHITE),
            ..container::Style::default()
        })
        .into()
}

/// 一排会随 `phase` 起伏的频谱条。
pub fn equalizer<'a>(
    phase: f32,
    count: usize,
    bar_width: f32,
    height: f32,
    color: iced::Color,
) -> Element<'a> {
    let mut bars = row![]
        .spacing(bar_width * 1.18)
        .height(height)
        .align_y(Alignment::End);

    for index in 0..count {
        let index_f = index as f32;
        let slow = (phase * 1.9 + index_f * 0.55).sin() * 0.5 + 0.5;
        let fast = (phase * 0.8 + index_f * 1.21).cos() * 0.5 + 0.5;
        let wave = (slow * 0.58 + fast * 0.42).clamp(0.0, 1.0);
        let min_height = height * 0.12;
        let bar_height = min_height + (height - min_height) * wave;
        let alpha = 0.26 + 0.60 * ((index_f + 1.0) / count as f32);

        let bar = container(row![])
            .width(bar_width)
            .height(bar_height)
            .style(move |_| container::Style {
                background: Some(Background::Color(iced::Color { a: alpha, ..color })),
                border: border::rounded(bar_width * 0.5),
                ..container::Style::default()
            });
        bars = bars.push(bar);
    }

    bars.into()
}

/// 用真实频谱数据画一排柱子（播放中）。数据不足时退化成最短的线。
pub fn equalizer_live<'a>(
    bands: &'a [f64],
    count: usize,
    bar_width: f32,
    height: f32,
    color: iced::Color,
) -> Element<'a> {
    let mut bars = row![]
        .spacing(bar_width * 0.9)
        .height(height)
        .align_y(Alignment::End);

    for index in 0..count {
        let level = if bands.is_empty() {
            0.0
        } else {
            let position = (index as f32 / count.max(1) as f32 * bands.len() as f32) as usize;
            bands[position.min(bands.len() - 1)].clamp(0.0, 1.0)
        };
        let bar_height = (height * (0.14 + 0.86 * level as f32)).max(2.0);
        let alpha = 0.55 + 0.45 * level as f32;

        let bar = container(row![])
            .width(bar_width)
            .height(bar_height)
            .style(move |_| container::Style {
                background: Some(Background::Color(iced::Color { a: alpha, ..color })),
                border: border::rounded(bar_width * 0.5),
                ..container::Style::default()
            });
        bars = bars.push(bar);
    }

    bars.into()
}

/// 呼吸状态点，用来提示“正在等待扫码”。
pub fn pulse_dot<'a>(color: iced::Color, phase: f32, size: f32) -> Element<'a> {
    let alpha = 0.25 + 0.75 * ((phase * 2.1).sin() * 0.5 + 0.5);
    container(row![])
        .width(size)
        .height(size)
        .style(move |_| container::Style {
            background: Some(Background::Color(iced::Color { a: alpha, ..color })),
            border: border::rounded(size * 0.5),
            ..container::Style::default()
        })
        .into()
}

pub fn feature_line_dark<'a>(value: &'a str) -> Element<'a> {
    row![
        dot(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.75), 6.0),
        text(value).size(12).color(theme::WHITE_82),
    ]
    .spacing(9)
    .align_y(Alignment::Center)
    .into()
}

/// 用几层同心圆伪造柔和光晕（iced 没有高斯模糊，只能叠透明度）。
pub fn glow(size: f32, color: iced::Color, strength: f32) -> Element<'static> {
    fn circle(size: f32, stage: f32, color: iced::Color, alpha: f32) -> Element<'static> {
        container(
            container(row![])
                .width(size)
                .height(size)
                .style(move |_| container::Style {
                    background: Some(Background::Color(iced::Color { a: alpha, ..color })),
                    border: border::rounded(size * 0.5),
                    ..container::Style::default()
                }),
        )
        .width(stage)
        .height(stage)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
    }

    iced::widget::stack![
        circle(size, size, color, strength * 0.22),
        circle(size * 0.72, size, color, strength * 0.42),
        circle(size * 0.44, size, color, strength * 0.75),
    ]
    .into()
}

pub fn gradient_rule<'a>(height: f32) -> Element<'a> {
    container(row![])
        .width(Length::Fill)
        .height(height)
        .style(move |_| container::Style {
            background: Some(Background::Color(iced::Color::from_rgba(
                0.231, 0.447, 1.0, 0.35,
            ))),
            border: border::rounded(height * 0.5),
            ..container::Style::default()
        })
        .into()
}

pub fn pill<'a>(label: impl Into<String>, kind: PillKind) -> Element<'a> {
    let (background, border_color, text_color) = match kind {
        PillKind::Ok => (
            iced::Color::from_rgb(0.949, 0.984, 0.961),
            iced::Color::from_rgb(0.718, 0.886, 0.776),
            theme::OK,
        ),
        PillKind::Warn => (
            iced::Color::from_rgb(1.0, 0.980, 0.941),
            iced::Color::from_rgb(1.0, 0.835, 0.569),
            theme::WARN,
        ),
        PillKind::Muted => (
            iced::Color::from_rgb(0.961, 0.969, 0.980),
            theme::LINE,
            theme::DIM,
        ),
        PillKind::Like => (
            iced::Color::from_rgb(1.0, 0.949, 0.957),
            iced::Color::from_rgb(1.0, 0.761, 0.792),
            theme::LIKE,
        ),
    };

    container(text(label.into()).size(11).color(text_color))
        .padding(Padding::from([2, 8]))
        .style(move |_| container::Style {
            background: Some(Background::Color(background)),
            border: border::rounded(999.0).width(1.0).color(border_color),
            text_color: Some(text_color),
            ..container::Style::default()
        })
        .into()
}

pub fn dot(color: iced::Color, size: f32) -> Element<'static> {
    container(row![])
        .width(size)
        .height(size)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: border::rounded(size * 0.5),
            ..container::Style::default()
        })
        .into()
}

pub fn section_title<'a>(title: impl Into<String>) -> Element<'a> {
    text(title.into()).size(14).color(theme::ACCENT_DARK).into()
}

pub fn hint<'a>(value: impl Into<String>) -> Element<'a> {
    text(value.into()).size(11).color(theme::DIM2).into()
}

pub fn error_text<'a>(value: impl Into<String>) -> Element<'a> {
    text(value.into()).size(11).color(theme::ERR).into()
}

pub fn separator<'a>() -> Element<'a> {
    container(row![])
        .height(1)
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(theme::LINE_SOFT)),
            ..container::Style::default()
        })
        .into()
}

pub fn vspace<'a>(height: f32) -> Element<'a> {
    vertical_space().height(height).into()
}

pub fn primary_button<'a>(
    label: &'a str,
    icon: Option<&'static [u8]>,
    message: Message,
) -> Element<'a> {
    let content: Element<'a> = match icon {
        Some(icon) => row![
            svg_icon(icon, 13.0, iced::Color::WHITE),
            text(label).size(13).color(iced::Color::WHITE),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
        None => text(label).size(13).color(iced::Color::WHITE).into(),
    };

    button(content)
        .on_press(message)
        .padding(Padding::from([8, 14]))
        .width(Length::Fill)
        .style(theme::primary)
        .into()
}

pub fn ghost_button<'a>(
    label: &'a str,
    icon: Option<&'static [u8]>,
    message: Message,
) -> Element<'a> {
    let content: Element<'a> = match icon {
        Some(icon) => row![
            svg_icon(icon, 12.0, theme::FG),
            text(label).size(12).color(theme::FG),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .into(),
        None => text(label).size(12).color(theme::FG).into(),
    };

    button(content)
        .on_press(message)
        .padding(Padding::from([6, 11]))
        .style(theme::ghost)
        .into()
}

/// 侧边栏里的导航项。第一步只做登录，因此除“我的歌单”外都标记为下一步。
pub fn nav_display(
    title: &'static str,
    subtitle: &'static str,
    icon: &'static [u8],
    active: bool,
    upcoming: bool,
) -> Element<'static> {
    let title_color = if active {
        iced::Color::WHITE
    } else {
        theme::FG
    };
    let subtitle_color = if active {
        iced::Color::from_rgba(1.0, 1.0, 1.0, 0.78)
    } else {
        theme::DIM2
    };
    let icon_color = if active {
        iced::Color::WHITE
    } else {
        theme::DIM
    };

    let mut content = row![
        svg_icon(icon, 18.0, icon_color),
        iced::widget::column![
            text(title).size(13).color(title_color),
            text(subtitle).size(10).color(subtitle_color)
        ]
        .spacing(1),
        iced::widget::horizontal_space(),
    ]
    .spacing(11)
    .align_y(Alignment::Center);

    if upcoming {
        content = content.push(pill("下一步", PillKind::Muted));
    }

    container(content)
        .width(Length::Fill)
        .height(Length::Fixed(50.0))
        .padding(Padding::from([0, 11]))
        .align_y(Alignment::Center)
        .style(move |_| {
            if active {
                container::Style {
                    background: Some(theme::button_gradient()),
                    border: border::rounded(9.0),
                    text_color: Some(iced::Color::WHITE),
                    shadow: Shadow {
                        color: iced::Color::from_rgba(0.231, 0.447, 1.0, 0.28),
                        offset: iced::Vector::new(0.0, 4.0),
                        blur_radius: 12.0,
                    },
                }
            } else {
                container::Style {
                    background: Some(Background::Color(theme::TRANSPARENT)),
                    border: border::rounded(9.0),
                    text_color: Some(theme::FG),
                    shadow: Shadow::default(),
                }
            }
        })
        .into()
}

/// 圆形头像：下载后的图片已经在本层裁成带透明通道的 PNG。
pub fn avatar<'a>(app: &'a App, size: f32) -> Element<'a> {
    if let Some(handle) = &app.avatar {
        image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .into()
    } else {
        let initial = app
            .profile
            .as_ref()
            .map(nm_core::Profile::initial)
            .unwrap_or_else(|| "?".to_string());

        container(text(initial).size(size * 0.40).color(iced::Color::WHITE))
            .width(size)
            .height(size)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |_| container::Style {
                background: Some(theme::brand_gradient()),
                border: border::rounded(size * 0.5),
                ..container::Style::default()
            })
            .into()
    }
}

/// 三点弹跳加载动画。`phase` 由 App 的定时 tick 推进。
pub fn spinner<'a>(phase: f32, size: f32, color: iced::Color) -> Element<'a> {
    let mut dots = row![].spacing(size * 0.7).align_y(Alignment::Center);
    for index in 0..3 {
        let wave = (phase - index as f32 * 0.7).sin() * 0.5 + 0.5;
        let alpha = 0.35 + 0.65 * wave;
        let dot = container(row![])
            .width(size)
            .height(size)
            .style(move |_| container::Style {
                background: Some(Background::Color(iced::Color { a: alpha, ..color })),
                border: border::rounded(size * 0.5),
                ..container::Style::default()
            });
        dots = dots.push(container(dot).height(size * 2.2).padding(Padding {
            top: (1.0 - wave) * size * 1.2,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }));
    }
    dots.into()
}

/// 统一的页面标题条：品牌色平铺窄条，只放标签、标题与副标题，
/// 右侧不作任何装饰。
pub fn title_bar<'a>(title: &'a str, subtitle: &'a str) -> Element<'a> {
    container(
        iced::widget::column![
            row![
                dot(iced::Color::WHITE, 5.0),
                text("MONTIS · 山音").size(9).color(theme::WHITE_70),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
            row![
                text(title).size(17).color(iced::Color::WHITE),
                text(subtitle).size(11).color(theme::WHITE_82),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        ]
        .spacing(3),
    )
    .width(Length::Fill)
    .padding(Padding::from([10, 18]))
    .style(|_| container::Style {
        background: Some(theme::hero_gradient()),
        border: border::rounded(12.0),
        ..container::Style::default()
    })
    .into()
}

/// 列表行之间的极淡分割线；两侧渐隐，避免把卡片切得太硬。
pub fn soft_rule<'a>() -> Element<'a> {
    container(row![])
        .width(Length::Fill)
        .height(1)
        .style(|_| container::Style {
            background: Some(theme::linear(
                90.0,
                &[
                    (0.0, iced::Color::from_rgba(0.55, 0.62, 0.78, 0.0)),
                    (0.18, iced::Color::from_rgba(0.55, 0.62, 0.78, 0.16)),
                    (0.82, iced::Color::from_rgba(0.55, 0.62, 0.78, 0.16)),
                    (1.0, iced::Color::from_rgba(0.55, 0.62, 0.78, 0.0)),
                ],
            )),
            ..container::Style::default()
        })
        .into()
}

// ==========================================================================
// 过渡：整体平移
// ==========================================================================

/// 把子树整体向下平移 `offset` 像素。
///
/// iced 0.13 没有位移 / 透明度组件；播放页的「控制台滑出」直接给渲染层
/// 加一个平移变换。`offset` 不为 0 时子树的交互全部忽略，避免滑出后
/// 在原位置留下看不见的点击区。
pub fn slide_down<'a>(content: impl Into<Element<'a>>, offset: f32) -> Element<'a> {
    Element::new(Slide {
        content: content.into(),
        offset,
    })
}

struct Slide<'a> {
    content: Element<'a>,
    offset: f32,
}

impl<'a> Widget<Message, iced::Theme, iced::Renderer> for Slide<'a> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &self,
        tree: &mut Tree,
        renderer: &iced::Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        renderer.with_translation(Vector::new(0.0, self.offset), |renderer| {
            self.content.as_widget().draw(
                &tree.children[0],
                renderer,
                theme,
                style,
                layout,
                cursor,
                viewport,
            );
        });
    }

    fn operate(
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
            .as_widget()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> iced::event::Status {
        if self.offset.abs() > 0.5 {
            return iced::event::Status::Ignored;
        }

        self.content.as_widget_mut().on_event(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        )
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        if self.offset.abs() > 0.5 {
            return mouse::Interaction::None;
        }

        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &iced::Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content
            .as_widget_mut()
            .overlay(&mut tree.children[0], layout, renderer, translation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nm_api::Artist;

    fn song(artists: &[(u64, &str)]) -> Song {
        Song {
            ar: artists
                .iter()
                .map(|(id, name)| Artist {
                    id: *id,
                    name: (*name).to_string(),
                })
                .collect(),
            ..Song::default()
        }
    }

    #[test]
    fn splits_every_artist_with_its_own_id() {
        let song = song(&[(1, "温岚"), (2, "周杰伦")]);
        let segments = artist_segments(&song, 20);
        assert_eq!(
            segments,
            vec![
                ("温岚".to_string(), Some(1)),
                ("周杰伦".to_string(), Some(2))
            ]
        );
    }

    #[test]
    fn truncates_multi_artist_names_inside_the_budget() {
        let song = song(&[(1, "一二三四"), (2, "五六七八")]);
        let segments = artist_segments(&song, 8);
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0], ("一二三四".to_string(), Some(1)));
        assert_eq!(segments[1].0, "五…");
        assert_eq!(segments[1].1, Some(2));
    }

    #[test]
    fn falls_back_to_unknown_artist() {
        let segments = artist_segments(&Song::default(), 10);
        assert_eq!(segments, vec![("未知歌手".to_string(), None)]);
    }
}

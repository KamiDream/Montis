//! 播放条与「正在播放」页（含实时频谱）。

use iced::alignment;
use iced::widget::canvas::{self, Canvas, Frame, Geometry, Path, Text};
use iced::widget::{
    button, column, container, horizontal_space, image, mouse_area, row, scrollable,
};
use iced::{
    border, mouse, Alignment, Background, Color, ContentFit, Length, Padding, Pixels, Point,
    Rectangle, Size,
};

use nm_api::Song;

use crate::app::{App, AudioQuality, Element, Message, RepeatMode, ScrollArea};
use crate::theme;
use crate::visualizer::{Palette, VisualField, VisualToggle, VisualizerConfig};
use crate::widgets::{
    self, text, ICON_BACK, ICON_EXPAND, ICON_NEXT, ICON_PAUSE, ICON_PLAY, ICON_PREV, ICON_QUEUE,
    ICON_SHUFFLE, ICON_VOLUME, ICON_VOLUME_MUTE,
};

/// 播放条高度；Home 布局里按固定高度扣掉。
pub const BAR_HEIGHT: f32 = 86.0;
/// 播放条次级控制键（随机 / 上一首 / 下一首 / 循环）的图标尺寸。
const BAR_CONTROL_ICON: f32 = 15.0;
/// 正在播放页次级控制键的图标尺寸。
const PAGE_CONTROL_ICON: f32 = 20.0;
/// 正在播放页底部「曲目信息 + 控制台」占用的高度（含底部留白）。
const INFO_STACK: f32 = 340.0;
/// 收起时整块信息 + 控制台向下滑出的距离。
///
/// 留出比信息栈更多的余量：歌词换行后控制台会变高，
/// 滑动距离不足时收起状态会露出控制台顶部。
const DOCK_SLIDE: f32 = INFO_STACK + 140.0;
/// 指针进入窗口底部这块区域时展开信息与控制台。
/// 必须盖住展开后的整块内容（含错误提示），否则指针停在歌名上时
/// 控制台会反复收起。
const DOCK_HOVER_HEIGHT: f32 = INFO_STACK + 110.0;
/// 悬停区比控制台两侧各宽出这么多，避免指针贴在控制台边缘时被判定为离开。
const DOCK_HOVER_MARGIN: f32 = 40.0;
// 滑出距离必须能把整块内容送出页面。
const _: () = assert!(DOCK_SLIDE > INFO_STACK);

/// 正在播放页底部信息与控制台的宽度。
pub(crate) fn dock_width(viewport_width: f32) -> f32 {
    (viewport_width * 0.60).clamp(520.0, 720.0)
}

/// 指针是否落在「展开底部信息 + 控制台」的悬停区里。
///
/// 播放页默认收起，指针移到窗口底部中央才滑出；窄窗口时悬停区
/// 跟着收窄，避免整条底边都触发。
pub(crate) fn pointer_hovers_dock(pointer: Point, viewport: Size) -> bool {
    let center_x = viewport.width * 0.5;
    let half_width = dock_width(viewport.width) * 0.5 + DOCK_HOVER_MARGIN;
    pointer.y >= viewport.height - DOCK_HOVER_HEIGHT && (pointer.x - center_x).abs() <= half_width
}

/// 正在播放页的环形画布尺寸，以及是否让它叠在曲目信息后面。
///
/// 窗口够高时画布铺满页面（不再有 600px 上限）；窗口偏矮时把画布收回到
/// 曲目信息上方，保证封面（第 6 个同心环）不会压在歌名上。
/// `cover_ratio` 是封面半径相对画布边长的比例。
fn stage_layout(page_width: f32, page_height: f32, cover_ratio: f32) -> (f32, bool) {
    let full = page_width.min(page_height);
    let info_top = (page_height - INFO_STACK).max(0.0);
    if page_height * 0.5 + cover_ratio * full <= info_top {
        (full, true)
    } else {
        // 封面圆心在画布中心：ring / 2 + ratio * ring 不越过信息区。
        let limit = info_top / (0.5 + cover_ratio.max(0.0));
        (page_width.min(limit).max(240.0), false)
    }
}

/// 把舞台大小的内容（光晕 / 封面）按原来的位置放进窗口。
fn stage_slot<'a>(content: Element<'a>, overlay: bool) -> Element<'a> {
    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding::from([18, 30]))
        .align_x(Alignment::Center)
        .align_y(if overlay {
            Alignment::Center
        } else {
            Alignment::Start
        })
        .into()
}

/// 频谱舞台的尺寸与环心位置（窗口坐标）。
///
/// 画布现在铺满整个窗口，环心仍按原来的布局计算；粒子裁剪也用它，
/// 保证「只在飞出屏幕时消失」与真实画布一致。
pub(crate) struct StageMetrics {
    pub size: f32,
    pub center: Point,
    pub overlay: bool,
}

pub(crate) fn stage_metrics(viewport: Size, config: &VisualizerConfig) -> StageMetrics {
    let page_width = (viewport.width - 60.0).max(240.0);
    let page_height = (viewport.height - 36.0).max(240.0);
    let cover_ratio = crate::viz::center_ring_radius(1.0, config, 6);
    let (size, overlay) = stage_layout(page_width, page_height, cover_ratio);
    let center = Point::new(
        viewport.width * 0.5,
        if overlay {
            viewport.height * 0.5
        } else {
            // 偏矮的窗口：舞台贴顶，和 stage_position 的 Start 对齐一致。
            18.0 + size * 0.5
        },
    );

    StageMetrics {
        size,
        center,
        overlay,
    }
}

// ==========================================================================
// 进度 / 音量滑杆
// ==========================================================================

#[derive(Clone, Copy)]
struct BarColors {
    rail: Color,
    fill: Color,
    knob: Color,
    label: Color,
}

impl BarColors {
    fn light() -> Self {
        Self {
            rail: Color::from_rgba(0.17, 0.24, 0.31, 0.20),
            fill: theme::ACCENT,
            knob: theme::ACCENT,
            label: theme::DIM,
        }
    }
}

/// 控制台滑动用的缓入缓出：两端减速，起停不突兀。
fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// 两个颜色按 `t` 线性混合，用于加载动画的渐变拖尾。
fn mix_color(from: Color, to: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color {
        r: from.r + (to.r - from.r) * t,
        g: from.g + (to.g - from.g) * t,
        b: from.b + (to.b - from.b) * t,
        a: from.a + (to.a - from.a) * t,
    }
}

/// 加载态：`clock` 驱动动画，`mix` 是 0~1 的权重（淡入 / 淡出）。
#[derive(Clone, Copy)]
struct Loading {
    clock: f32,
    mix: f32,
}

/// 把 App 的加载状态整理成滑杆需要的参数。
fn loading_layer(app: &App) -> Option<Loading> {
    if !app.player.loading && app.load_mix <= 0.0 {
        return None;
    }
    Some(Loading {
        clock: app.clock,
        mix: app.load_mix,
    })
}

/// 可点击 / 拖动的滑杆。`on_change` 收到 0~1。
struct SeekBar<'a> {
    value: f32,
    thickness: f32,
    knob: f32,
    colors: BarColors,
    /// 左 / 右时间文案；两侧预留 `inset` 宽度。
    labels: Option<(String, String)>,
    label_size: f32,
    inset: f32,
    /// `Some` 时叠加不确定进度光效（正在加载音频），此时不可拖动。
    loading: Option<Loading>,
    on_change: Box<dyn Fn(f32) -> Message + 'a>,
}

impl canvas::Program<Message> for SeekBar<'_> {
    type State = bool;

    fn update(
        &self,
        dragging: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        let value_at = |cursor: mouse::Cursor| {
            cursor
                .position_in(bounds)
                .map(|position| {
                    let inset = self.inset.min(bounds.width / 2.0);
                    let rail = (bounds.width - inset * 2.0).max(1.0);
                    ((position.x - inset) / rail).clamp(0.0, 1.0)
                })
                .map(|value| (self.on_change)(value))
        };

        // 加载中没有可跳转的进度，拖动 / 点击一律忽略。
        if self.loading.is_some() {
            *dragging = false;
            return (canvas::event::Status::Ignored, None);
        }

        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                if let Some(message) = value_at(cursor) {
                    *dragging = true;
                    return (canvas::event::Status::Captured, Some(message));
                }
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                *dragging = false;
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) if *dragging => {
                if let Some(message) = value_at(cursor) {
                    return (canvas::event::Status::Captured, Some(message));
                }
            }
            _ => {}
        }

        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _dragging: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let width = bounds.width.max(1.0);
        let height = bounds.height.max(1.0);
        let thickness = self.thickness.min(height);
        let y = (height - thickness) / 2.0;
        let inset = self.inset.min(width / 2.0 - 1.0).max(0.0);
        let rail_width = (width - inset * 2.0).max(1.0);

        let mut frame = Frame::new(renderer, bounds.size());
        let radius = border::Radius::from(thickness / 2.0);

        let rail = Path::rounded_rectangle(
            Point::new(inset, y),
            Size::new(rail_width, thickness),
            radius,
        );

        let (clock, mix) = self.loading.map_or((0.0, 0.0), |loading| {
            (loading.clock, loading.mix.clamp(0.0, 1.0))
        });

        // 轨道底色：加载态缓慢呼吸，淡出时跟着回到常态。
        let breathe = 0.5 + 0.5 * (clock * 2.2).sin();
        let loading_rail = (self.colors.rail.a * (0.8 + 0.5 * breathe)).min(1.0);
        frame.fill(
            &rail,
            Color {
                a: self.colors.rail.a + (loading_rail - self.colors.rail.a) * mix,
                ..self.colors.rail
            },
        );

        // 正常进度与圆点：加载结束时淡入，避免进度条突然出现。
        if mix < 1.0 {
            let fade = 1.0 - mix;
            let filled = (rail_width * self.value.clamp(0.0, 1.0)).max(thickness);
            let value = Path::rounded_rectangle(
                Point::new(inset, y),
                Size::new(filled.min(rail_width), thickness),
                radius,
            );
            frame.fill(
                &value,
                Color {
                    a: fade,
                    ..self.colors.fill
                },
            );

            if self.knob > 0.0 {
                let x = (inset + rail_width * self.value.clamp(0.0, 1.0))
                    .clamp(inset + self.knob, inset + rail_width - self.knob);
                let center = Point::new(x, height / 2.0);
                // 圆点比填充出现得更晚，避免和淡出的彗星同时霸占视线。
                let knob_fade = fade * fade;
                frame.fill(
                    &Path::circle(center, self.knob),
                    Color::from_rgba(0.0, 0.0, 0.0, 0.12 * knob_fade),
                );
                frame.fill(
                    &Path::circle(center, self.knob - 1.5),
                    Color {
                        a: knob_fade,
                        ..self.colors.knob
                    },
                );
            }
        }

        // 不确定进度：让轨道底色缓慢呼吸，再叠加一颗往返滑动的「彗星」
        // （三层光晕 + 白核 + 渐隐尾巴）与一道缓慢扫过的柔光。
        if mix > 0.0 {
            // 背景扫光：偏青的低频光带掠过整条轨道，给静态底色一点流动感。
            let sweep_width = rail_width * 0.24;
            let sweep_x = inset - sweep_width + (clock / 4.7).fract() * (rail_width + sweep_width);
            let sweep_start = sweep_x.max(inset);
            let sweep_end = (sweep_x + sweep_width).min(inset + rail_width);
            if sweep_end > sweep_start {
                let sweep = Path::rounded_rectangle(
                    Point::new(sweep_start, y),
                    Size::new(sweep_end - sweep_start, thickness),
                    radius,
                );
                frame.fill(
                    &sweep,
                    Color {
                        a: 0.16 * mix,
                        ..theme::CYAN
                    },
                );
            }

            // 第二道反方向漂移的紫光，让轨道上始终有东西在流动。
            let reverse_width = rail_width * 0.18;
            let reverse_x =
                inset + rail_width - (clock / 6.3).fract() * (rail_width + reverse_width);
            let reverse_start = reverse_x.max(inset);
            let reverse_end = (reverse_x + reverse_width).min(inset + rail_width);
            if reverse_end > reverse_start {
                let sweep = Path::rounded_rectangle(
                    Point::new(reverse_start, y),
                    Size::new(reverse_end - reverse_start, thickness),
                    radius,
                );
                frame.fill(
                    &sweep,
                    Color {
                        a: 0.10 * mix,
                        ..theme::VIOLET
                    },
                );
            }

            // 彗星：余弦缓入缓出地往返，端点上速度最慢、尾巴最短。
            let phase = (clock / 2.6) * std::f32::consts::TAU;
            let swing = 0.5 - 0.5 * phase.cos();
            let core = (thickness * 0.9).max(1.5);
            let travel_left = inset + core;
            let travel_right = (inset + rail_width - core).max(travel_left);
            let head = travel_left + (travel_right - travel_left) * swing;
            let center = Point::new(head, height / 2.0);
            let speed = phase.sin();

            // 尾巴拖在运动方向的反面，长度随速度变化，避免掉头时抽动。
            let direction = if speed >= 0.0 { -1.0 } else { 1.0 };
            let tail_len = (rail_width * 0.62 * (0.38 + 0.62 * speed.abs()))
                .max(rail_width * 0.24)
                .max(thickness * 2.0)
                .min(rail_width);
            let steps = 18;
            for index in 0..steps {
                let start_f = index as f32 / steps as f32;
                let end_f = (index + 1) as f32 / steps as f32;
                let alpha = 0.6 * (1.0 - start_f).powf(1.6) * mix;
                if alpha <= 0.004 {
                    continue;
                }

                let x0 = head + direction * tail_len * start_f;
                let x1 = head + direction * tail_len * end_f;
                let start = x0.min(x1).max(inset);
                let end = x0.max(x1).min(inset + rail_width);
                if end <= start {
                    continue;
                }

                let segment = thickness * (1.0 - 0.55 * end_f).max(0.6);
                let path = Path::rounded_rectangle(
                    Point::new(start, height / 2.0 - segment / 2.0),
                    Size::new(end - start, segment),
                    border::Radius::from(segment / 2.0),
                );
                // 尾巴由本色渐变成品牌紫，拖得越远越冷。
                let tint = mix_color(self.colors.fill, theme::VIOLET, end_f * 0.85);
                frame.fill(&path, Color { a: alpha, ..tint });
            }

            // 紧贴头部的一小段亮芯，让彗尾有「实心拖尾」的质感。
            let trail = tail_len * 0.34;
            let start = head.min(head + direction * trail).max(inset);
            let end = head.max(head + direction * trail).min(inset + rail_width);
            if end > start {
                let core_bar = Path::rounded_rectangle(
                    Point::new(start, height / 2.0 - thickness * 0.28),
                    Size::new(end - start, thickness * 0.56),
                    border::Radius::from(thickness * 0.28),
                );
                frame.fill(
                    &core_bar,
                    Color {
                        a: 0.55 * mix,
                        ..mix_color(self.colors.fill, Color::WHITE, 0.55)
                    },
                );
            }

            // 紧贴头部的横向柔光，把亮点「拉」开成一团发光体。
            let glow_width = (thickness * 9.0).min(rail_width);
            let glow_height = thickness * 2.2;
            let glow_start = (head - glow_width / 2.0).max(inset);
            let glow_end = (head + glow_width / 2.0).min(inset + rail_width);
            if glow_end > glow_start {
                let head_glow = Path::rounded_rectangle(
                    Point::new(glow_start, height / 2.0 - glow_height / 2.0),
                    Size::new(glow_end - glow_start, glow_height),
                    border::Radius::from(glow_height / 2.0),
                );
                frame.fill(
                    &head_glow,
                    Color {
                        a: 0.12 * mix,
                        ..mix_color(self.colors.fill, Color::WHITE, 0.35)
                    },
                );
            }

            // 光晕：紫 → 蓝 → 近白逐层收紧，最后用本色画一个实心小核。
            let halo = (height / 2.0).max(core);
            let pulse = 1.0 + 0.06 * (clock * 4.0).sin();
            let layers = [
                (1.0, 0.10, theme::VIOLET),
                (0.76, 0.20, self.colors.fill),
                (0.50, 0.34, mix_color(self.colors.fill, Color::WHITE, 0.35)),
            ];
            for (scale, alpha, color) in layers {
                frame.fill(
                    &Path::circle(center, halo * scale * pulse),
                    Color {
                        a: alpha * mix,
                        ..color
                    },
                );
            }
            frame.fill(
                &Path::circle(center, core),
                Color {
                    a: mix,
                    ..self.colors.knob
                },
            );
        }

        if let Some((left, right)) = &self.labels {
            let text = |content: &str, position: Point, align_x: alignment::Horizontal| Text {
                content: content.to_string(),
                position,
                color: self.colors.label,
                size: Pixels(self.label_size),
                horizontal_alignment: align_x,
                vertical_alignment: alignment::Vertical::Center,
                ..Text::default()
            };

            frame.fill_text(text(
                left,
                Point::new(0.0, height / 2.0),
                alignment::Horizontal::Left,
            ));
            frame.fill_text(text(
                right,
                Point::new(width, height / 2.0),
                alignment::Horizontal::Right,
            ));
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        _bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        mouse::Interaction::Pointer
    }
}

/// 滑杆的外观尺寸。
#[derive(Clone, Copy)]
struct SeekStyle {
    height: f32,
    thickness: f32,
    knob: f32,
    colors: BarColors,
}

/// 滑杆两端的时间文案；`inset` 是单侧预留宽度。
struct SeekLabels {
    left: String,
    right: String,
    size: f32,
    inset: f32,
}

fn seek_bar<'a>(
    value: f32,
    style: SeekStyle,
    loading: Option<Loading>,
    labels: Option<SeekLabels>,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a> {
    let (labels, label_size, inset) = labels.map_or((None, 0.0, 0.0), |labels| {
        (Some((labels.left, labels.right)), labels.size, labels.inset)
    });

    Canvas::new(SeekBar {
        value,
        thickness: style.thickness,
        knob: style.knob,
        colors: style.colors,
        labels,
        label_size,
        inset,
        loading,
        on_change: Box::new(on_change),
    })
    .width(Length::Fill)
    .height(Length::Fixed(style.height))
    .into()
}

/// 设置页复用的小滑杆：浅色主题、无标签，`value` 为 0~1。
pub(crate) fn setting_slider<'a>(
    value: f32,
    on_change: impl Fn(f32) -> Message + 'a,
) -> Element<'a> {
    seek_bar(
        value,
        SeekStyle {
            height: 20.0,
            thickness: 4.0,
            knob: 5.0,
            colors: BarColors::light(),
        },
        None,
        None,
        on_change,
    )
}

// ==========================================================================
// 底部播放条
// ==========================================================================

pub fn bar(app: &App) -> Element<'_> {
    let Some(song) = &app.player.current else {
        return row![].into();
    };

    let progress = app.player.progress();
    let playing = app.player.is_playing();

    let left = row![
        bar_cover(app, song, 46.0),
        column![
            row![
                text(clip(&song.name, 22)).size(13).color(theme::FG),
                widgets::download_button(app, song, 13.0, false),
                widgets::like_button(
                    app.is_liked(song.id),
                    13.0,
                    theme::DIM2,
                    Message::ToggleLike(song.clone()),
                ),
            ]
            .spacing(4)
            .align_y(Alignment::Center),
            row![
                text(clip(&song.artist_names(), 26))
                    .size(10)
                    .color(theme::DIM),
                // 加载进度由进度条的滑动动画表达，不再在这里堆文字。
                if app.player.error.is_some() {
                    widgets::error_text("播放失败")
                } else {
                    row![].into()
                },
                quality_pill(app, false),
                quality_note(app, false),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        ]
        .spacing(3),
    ]
    .spacing(10)
    .align_y(Alignment::Center);

    let controls = row![
        mode_icon_button(
            ICON_SHUFFLE,
            app.player.mode.shuffle,
            BAR_CONTROL_ICON,
            theme::DIM,
            Message::ToggleShuffle,
        ),
        icon_button(
            ICON_PREV,
            BAR_CONTROL_ICON,
            theme::DIM,
            Message::PrevTrack,
            false
        ),
        icon_button(
            if playing { ICON_PAUSE } else { ICON_PLAY },
            16.0,
            Color::WHITE,
            Message::PlayPause,
            true
        ),
        icon_button(
            ICON_NEXT,
            BAR_CONTROL_ICON,
            theme::DIM,
            Message::NextTrack,
            false
        ),
        mode_icon_button(
            widgets::repeat_icon(app.player.mode.repeat),
            app.player.mode.repeat != RepeatMode::Off,
            BAR_CONTROL_ICON,
            theme::DIM,
            Message::CycleRepeat,
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let center = column![
        controls,
        container(seek_bar(
            progress,
            SeekStyle {
                height: 16.0,
                thickness: 4.0,
                knob: 4.0,
                colors: BarColors::light(),
            },
            loading_layer(app),
            Some(SeekLabels {
                left: format_time(app.player.position),
                right: format_time(app.player.duration),
                size: 11.0,
                inset: 62.0,
            }),
            Message::SeekTo
        ))
        .width(Length::Fixed(300.0)),
    ]
    .spacing(2)
    .align_x(Alignment::Center);

    let volume = row![
        volume_control(app, BAR_CONTROL_ICON, theme::DIM),
        icon_button(
            ICON_QUEUE,
            BAR_CONTROL_ICON,
            theme::DIM,
            Message::ToggleQueue,
            false
        ),
        icon_button(
            ICON_EXPAND,
            BAR_CONTROL_ICON,
            theme::DIM,
            Message::OpenNowPlaying,
            false
        ),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let sides = row![left, horizontal_space(), volume]
        .spacing(14)
        .align_y(Alignment::Center)
        .height(Length::Fill);

    // 中间控制区叠在整条播放条上单独居中：左右两侧宽度不同时，
    // 用 row + 两侧占位会把播放键推偏。
    let centered = container(center)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center);

    let content = iced::widget::stack![sides, centered]
        .width(Length::Fill)
        .height(Length::Fill);

    let inner = container(content)
        .width(Length::Fill)
        .height(Length::Fixed(BAR_HEIGHT - 2.0))
        .padding(Padding::from([10, 18]));

    let body = container(column![
        widgets::gradient_rule(2.0),
        inner.align_y(Alignment::Center),
    ])
    .width(Length::Fill)
    .height(Length::Fixed(BAR_HEIGHT))
    .style(theme::player_bar);

    mouse_area(body)
        .on_press(Message::OpenNowPlaying)
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn bar_cover<'a>(app: &'a App, song: &'a Song, size: f32) -> Element<'a> {
    let url = song.cover(600);
    let art: Element<'a> = match app.covers.get(&url) {
        Some(handle) => image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .into(),
        None => container(text("♪").size(size * 0.4).color(theme::WHITE_82))
            .width(size)
            .height(size)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(|_| container::Style {
                background: Some(theme::brand_gradient()),
                border: border::rounded(10.0),
                ..container::Style::default()
            })
            .into(),
    };

    container(art)
        .style(|_| container::Style {
            border: border::rounded(10.0),
            ..container::Style::default()
        })
        .into()
}

// ==========================================================================
// 正在播放页
// ==========================================================================

pub fn page(app: &App) -> Element<'_> {
    let Some(song) = &app.player.current else {
        return container(widgets::hint("还没有正在播放的歌曲")).into();
    };

    let backdrop: Element<'_> = match app.covers.get(&song.cover(600)) {
        Some(handle) => image(handle.clone())
            .width(Length::Fill)
            .height(Length::Fill)
            .content_fit(ContentFit::Cover)
            .opacity(0.45_f32)
            .into(),
        None => container(row![])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| container::Style {
                background: Some(theme::brand_gradient()),
                ..container::Style::default()
            })
            .into(),
    };

    let scrim = container(row![])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(theme::scrim);

    // 频谱画布铺满整个窗口：环的大小仍按舞台边长计算，但粒子 / 辉光可以一直画到
    // 屏幕边缘。窗口够高时环心在页面中间；偏矮时舞台贴顶，环心跟着上移。
    let metrics = stage_metrics(app.viewport, &app.visual);
    let ring_size = metrics.size;
    let overlay = metrics.overlay;
    // 封面裁成圆，圆边正好落在中心组件「从外往里第 6 个」同心环上。
    // 条高拉到 100% 以上时中心环会收到圆心，给封面留一个最小尺寸，
    // 否则会出现 0 尺寸图片 / 0 号字导致排版崩溃。
    // 低频起伏时用和中心圆相同的系数缩放，圆边始终贴着第 6 个环；封面本身不旋转。
    let cover_base = crate::viz::center_ring_radius(ring_size, &app.visual, 6) * 2.0;
    let pulse = crate::viz::center_scale(app.ring.center_bass());
    let cover_size = (cover_base * pulse).max(72.0);
    // 占位音符不跟着律动：iced 0.13 的文本在每帧都变的布局尺寸下会丢字形，
    // 给文本一个固定大小的盒子，圆本身继续平滑缩放。
    let cover_note = cover_base.max(72.0) * 0.25;

    // 光晕在下、画布在中、封面在上：三层分开，画布才能铺满窗口，
    // 而光晕 / 封面仍按原来的舞台位置摆放。
    let halo = stage_slot(cover_halo(app, cover_size, ring_size), overlay);
    let viz = crate::viz::widget(
        &app.ring,
        &app.visual,
        ring_size,
        [0.0, metrics.center.y - app.viewport.height * 0.5],
    )
    .width(Length::Fill)
    .height(Length::Fill);
    let cover = stage_slot(
        container(big_cover(app, song, cover_size, cover_note))
            .width(Length::Fixed(ring_size))
            .height(Length::Fixed(ring_size))
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .into(),
        overlay,
    );

    let info = column![
        text("NOW PLAYING").size(10).color(theme::WHITE_70),
        text(clip(&song.name, 30)).size(28).color(Color::WHITE),
        widgets::artist_link(song, 42, 13.0, theme::WHITE_82),
        text(format!("《{}》", clip(song.album_name(), 28)))
            .size(11)
            .color(theme::WHITE_70),
        row![
            quality_pill(app, true),
            quality_note(app, true),
            widgets::brand_pill(format!(
                "第 {} / {} 首",
                app.player.index + 1,
                app.player.queue.len()
            )),
        ]
        .spacing(8),
        if let Some(error) = &app.player.error {
            error_banner(error)
        } else {
            row![].into()
        },
    ]
    .spacing(7)
    .align_x(Alignment::Center);

    let timing = seek_bar(
        app.player.progress(),
        SeekStyle {
            height: 22.0,
            thickness: 5.0,
            knob: 6.0,
            colors: BarColors::light(),
        },
        loading_layer(app),
        Some(SeekLabels {
            left: format_time(app.player.position),
            right: format_time(app.player.duration),
            size: 11.0,
            inset: 62.0,
        }),
        Message::SeekTo,
    );

    let transport = row![
        mode_icon_button(
            ICON_SHUFFLE,
            app.player.mode.shuffle,
            PAGE_CONTROL_ICON,
            theme::DIM,
            Message::ToggleShuffle,
        ),
        icon_button(
            ICON_PREV,
            PAGE_CONTROL_ICON,
            theme::DIM,
            Message::PrevTrack,
            false
        ),
        icon_button(
            if app.player.is_playing() {
                ICON_PAUSE
            } else {
                ICON_PLAY
            },
            26.0,
            Color::WHITE,
            Message::PlayPause,
            true
        ),
        icon_button(
            ICON_NEXT,
            PAGE_CONTROL_ICON,
            theme::DIM,
            Message::NextTrack,
            false
        ),
        mode_icon_button(
            widgets::repeat_icon(app.player.mode.repeat),
            app.player.mode.repeat != RepeatMode::Off,
            PAGE_CONTROL_ICON,
            theme::DIM,
            Message::CycleRepeat,
        ),
    ]
    .spacing(14)
    .align_y(Alignment::Center);

    // 左右两侧都占满剩余宽度：音量靠左，队列按钮靠右，中间控制键绝对居中。
    let controls = row![
        container(volume_control(app, PAGE_CONTROL_ICON, theme::DIM))
            .width(Length::Fill)
            .align_x(Alignment::Start),
        transport,
        container(
            row![
                widgets::download_button(app, song, 16.0, false),
                widgets::like_button(
                    app.is_liked(song.id),
                    16.0,
                    theme::WHITE_70,
                    Message::ToggleLike(song.clone()),
                ),
                icon_button(
                    ICON_QUEUE,
                    PAGE_CONTROL_ICON,
                    theme::DIM,
                    Message::ToggleQueue,
                    false
                ),
            ]
            .spacing(10)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .align_x(Alignment::End),
    ]
    .align_y(Alignment::Center);

    let top = row![
        // 用真正的按钮而不是裸 `mouse_area`：命中区 = 内容 + padding，
        // 并且悬停时会有反馈，不会让人觉得「点了没反应」。
        button(
            row![
                widgets::svg_icon(ICON_BACK, 15.0, Color::WHITE),
                text("返回").size(12).color(theme::WHITE_82),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .on_press(Message::CloseNowPlaying)
        .padding(Padding::from([7, 13]))
        .style(theme::ghost_pill),
        horizontal_space(),
    ]
    .spacing(7)
    .align_y(Alignment::Center);

    let dock_w = dock_width(app.viewport.width);

    let mut dock_column = column![container(widgets::gradient_rule(2.0))
        .width(Length::Fixed(56.0))
        .align_x(Alignment::Center),]
    .spacing(12)
    .width(Length::Fill)
    .align_x(Alignment::Center);

    if app.ui.lyrics {
        dock_column = dock_column.push(lyrics_block(app));
    }
    dock_column = dock_column.push(timing).push(controls);

    let dock = container(dock_column)
        .width(Length::Fixed(dock_w))
        .padding(Padding::from([16, 26]))
        .style(theme::dock(app.ui.dock_opacity));

    // 曲目信息与控制台是同进同出的一整块：默认全部收起，只留底部小进度条；
    // 指针移到窗口底部中央时整块滑上来。收起时只平移出屏，布局高度不变。
    // 缓动后两端更柔和，线性平移在 60fps 下也会显得一顿一顿。
    let dock_mix = ease_in_out(app.dock_mix);
    let dock_offset = (1.0 - dock_mix) * DOCK_SLIDE;
    let panel = column![info, dock]
        .spacing(14)
        .width(Length::Fill)
        .align_x(Alignment::Center);
    let bottom: Element<'_> = iced::widget::stack![
        widgets::slide_down(
            container(panel)
                .width(Length::Fill)
                .align_x(Alignment::Center),
            dock_offset,
        ),
        container(mini_lyric(app, 1.0 - dock_mix))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::End)
            .padding(Padding {
                top: 0.0,
                right: 0.0,
                bottom: 34.0,
                left: 0.0,
            }),
        container(mini_progress(app, 1.0 - dock_mix))
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::Center)
            .align_y(Alignment::End)
            .padding(Padding {
                top: 0.0,
                right: 0.0,
                bottom: 10.0,
                left: 0.0,
            }),
    ]
    .into();

    let content = column![top, iced::widget::vertical_space(), bottom]
        .spacing(14)
        .width(Length::Fill)
        .height(Length::Fill)
        .align_x(Alignment::Center);

    // 底部可读性渐变：频谱会跑到曲目信息与控制台后面，压暗下半屏保证白字可读。
    // 收起时跟着淡出，只留一层很淡的底影给底部小进度条。
    let shade = 0.30 + 0.70 * dock_mix;
    let dim = move |alpha: f32| Color {
        a: alpha * shade,
        ..theme::NIGHT
    };
    let readability = container(row![])
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(theme::linear(
                180.0,
                &[
                    (0.0, Color::TRANSPARENT),
                    (0.36, Color::TRANSPARENT),
                    (0.58, dim(0.30)),
                    (0.72, dim(0.78)),
                    (1.0, dim(0.96)),
                ],
            )),
            ..container::Style::default()
        });

    let layers: Vec<Element<'_>> = vec![
        backdrop,
        scrim.into(),
        halo,
        viz.into(),
        cover,
        readability.into(),
        container(content)
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding::from([18, 30]))
            .into(),
    ];

    iced::widget::Stack::with_children(layers).into()
}

/// 控制台里的歌词：当前行 + 翻译，换句时交叉淡化并轻微上浮。
fn lyrics_block(app: &App) -> Element<'_> {
    let content: Element<'_> = if app.lyrics.busy {
        text("歌词加载中…").size(11).color(theme::DIM).into()
    } else if app.lyrics.index.is_some() {
        let eased = ease_in_out(app.lyrics.mix.clamp(0.0, 1.0));
        let mut layers: Vec<Element<'_>> = Vec::new();

        // 旧句淡出；新句淡入，换句时整体更淡。
        if eased < 1.0 {
            let fade = (1.0 - eased) * (1.0 - eased);
            if let Some(previous) = lyric_body(app, app.lyrics.previous, fade) {
                layers.push(previous);
            }
        }
        if let Some(current) = lyric_body(app, app.lyrics.index, eased * eased) {
            layers.push(current);
        }

        iced::widget::Stack::with_children(layers).into()
    } else {
        let message = if app
            .player
            .current
            .as_ref()
            .is_some_and(|song| song.is_bilibili())
        {
            "B 站歌曲暂无歌词"
        } else if app.lyrics.error.is_some() {
            "歌词加载失败"
        } else {
            "暂无歌词"
        };
        text(message).size(11).color(theme::DIM).into()
    };

    container(content)
        .width(Length::Fill)
        .height(Length::Shrink)
        .padding(Padding::from([4, 0]))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

/// 一行歌词（含翻译）的完整内容，`alpha` 用于交叉淡化。
fn lyric_body<'a>(app: &'a App, index: Option<usize>, alpha: f32) -> Option<Element<'a>> {
    let line = app.lyrics.lyrics.lines.get(index?)?;
    let alpha = alpha.clamp(0.0, 1.0);
    let mut content = column![lyric_text(&line.text, 15.0, with_alpha(theme::FG, alpha))]
        .spacing(2)
        .width(Length::Fill)
        .align_x(Alignment::Center);
    if let Some(translation) = &line.translation {
        content = content.push(lyric_text(translation, 10.0, with_alpha(theme::DIM, alpha)));
    }

    Some(content.into())
}

/// 歌词文本：占满可用宽度并自动换行，长句不会被截断。
fn lyric_text<'a>(value: &'a str, size: f32, color: Color) -> iced::widget::Text<'a> {
    text(value)
        .size(size)
        .color(color)
        .width(Length::Fill)
        .center()
        .wrapping(iced::widget::text::Wrapping::WordOrGlyph)
}

fn with_alpha(color: Color, alpha: f32) -> Color {
    Color {
        a: color.a * alpha.clamp(0.0, 1.0),
        ..color
    }
}

/// 控制台收起时，小进度条上方的一行当前歌词。
fn mini_lyric<'a>(app: &'a App, fade: f32) -> Element<'a> {
    let fade = fade.clamp(0.0, 1.0);
    if !app.ui.lyrics || fade <= 0.01 {
        return container(row![]).height(Length::Fixed(0.0)).into();
    }

    let eased = ease_in_out(app.lyrics.mix.clamp(0.0, 1.0));
    let mut layers: Vec<Element<'a>> = Vec::new();

    if eased < 1.0 {
        let fade = fade * (1.0 - eased) * (1.0 - eased);
        if let Some(previous) = mini_line(app, app.lyrics.previous, fade) {
            layers.push(previous);
        }
    }
    if let Some(current) = mini_line(app, app.lyrics.index, fade * eased * eased) {
        layers.push(current);
    }

    if layers.is_empty() {
        return container(row![]).height(Length::Fixed(0.0)).into();
    }

    container(iced::widget::Stack::with_children(layers))
        .width(Length::Fill)
        .padding(Padding::from([0, 30]))
        .into()
}

fn mini_line<'a>(app: &'a App, index: Option<usize>, alpha: f32) -> Option<Element<'a>> {
    let line = app.lyrics.lyrics.lines.get(index?)?;
    let color = Color {
        a: 0.92 * alpha.clamp(0.0, 1.0),
        ..Color::WHITE
    };

    let mut content = column![lyric_text(&line.text, 14.0, color)]
        .spacing(1)
        .width(Length::Fill)
        .align_x(Alignment::Center);
    // 译文开关只影响控制台收起后的这行歌词；展开的控制台始终带译文。
    if app.ui.lyric_translation {
        if let Some(translation) = &line.translation {
            let color = Color {
                a: 0.70 * alpha.clamp(0.0, 1.0),
                ..Color::WHITE
            };
            content = content.push(lyric_text(translation, 10.0, color));
        }
    }

    Some(content.into())
}

/// 控制台收起后底部的小进度条：收起时也能看到播放进度。
fn mini_progress<'a>(app: &App, fade: f32) -> Element<'a> {
    const WIDTH: f32 = 200.0;
    let fade = fade.clamp(0.0, 1.0);
    let progress = app.player.progress().clamp(0.0, 1.0);
    let filled = (WIDTH * progress).max(2.0);

    let fill = container(row![])
        .width(Length::Fixed(filled.min(WIDTH)))
        .height(Length::Fill)
        .style(move |_| container::Style {
            background: Some(Background::Color(Color {
                a: 0.92 * fade,
                ..Color::WHITE
            })),
            border: border::rounded(2.5),
            ..container::Style::default()
        });

    let track = container(
        row![fill, horizontal_space()]
            .height(Length::Fill)
            .align_y(Alignment::Center),
    )
    .width(Length::Fixed(WIDTH))
    .height(Length::Fixed(5.0))
    .style(move |_| container::Style {
        background: Some(Background::Color(Color {
            a: 0.30 * fade,
            ..Color::WHITE
        })),
        border: border::rounded(2.5),
        ..container::Style::default()
    });

    container(track).padding(Padding::from([10, 48])).into()
}

// ==========================================================================
// 可视化设置面板
// ==========================================================================

pub(crate) fn settings_panel(app: &App) -> Element<'_> {
    let body = column![
        row![
            column![
                row![
                    widgets::dot(theme::ACCENT, 5.0),
                    text("VISUALIZER").size(9).color(theme::DIM2),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                text("可视化设置").size(16).color(theme::FG),
            ]
            .spacing(3),
            horizontal_space(),
            mouse_area(text("关闭").size(11).color(theme::DIM))
                .on_press(Message::ToggleSettings)
                .interaction(mouse::Interaction::Pointer),
        ]
        .align_y(Alignment::Center),
        widgets::gradient_rule(2.0),
        widgets::hint("改动会实时保存到 ~/.config/melody/visualizer.json"),
        section("形状"),
        slider_row(app, "条数", VisualField::Bars),
        slider_row(app, "环大小", VisualField::Size),
        slider_row(app, "条高", VisualField::Height),
        slider_row(app, "强度", VisualField::Intensity),
        section("频段"),
        slider_row(app, "低音", VisualField::Bass),
        slider_row(app, "中音", VisualField::Mid),
        slider_row(app, "高音", VisualField::Treble),
        section("动态"),
        toggle_row("自转", VisualToggle::Rotate, app.visual.rotate),
        slider_row(app, "自转速度", VisualField::RotationSpeed),
        toggle_row("顶端连线", VisualToggle::Links, app.visual.links),
        section("效果"),
        slider_row(app, "条辉光", VisualField::Glow),
        slider_row(app, "中心辉光", VisualField::CenterGlow),
        slider_row(app, "余晖", VisualField::Afterglow),
        slider_row(app, "CRT 强度", VisualField::Crt),
        slider_row(app, "桶形畸变", VisualField::Barrel),
        slider_row(app, "RGB 色散", VisualField::Aberration),
        section("音条粒子"),
        toggle_row("音条粒子", VisualToggle::Particles, app.visual.particles),
        slider_row(app, "粒子密度", VisualField::ParticleAmount),
        slider_row(app, "粒子速度", VisualField::ParticleSpeed),
        slider_row(app, "粒子寿命", VisualField::ParticleLife),
        section("鼓点爆发"),
        toggle_row("鼓点爆发", VisualToggle::Burst, app.visual.burst),
        slider_row(app, "鼓点灵敏度", VisualField::Sensitivity),
        slider_row(app, "爆发速度", VisualField::BurstSpeed),
        slider_row(app, "爆发寿命", VisualField::BurstLife),
        section("渲染"),
        toggle_row(
            "高刷新率 120fps",
            VisualToggle::HighFps,
            app.visual.high_fps
        ),
        section("配色"),
        palette_row(app),
        button(text("恢复默认").size(12).color(theme::FG))
            .on_press(Message::ResetVisual)
            .padding(Padding::from([8, 14]))
            .width(Length::Fill)
            .style(theme::ghost_soft),
    ]
    .spacing(12)
    .width(Length::Fill);

    let card = container(
        scrollable(
            container(body)
                .width(Length::Fill)
                // iced 的滚动条是覆盖在内容上的，右侧留出滚动条宽度，避免压住数值文字。
                .padding(Padding {
                    top: 0.0,
                    right: 14.0,
                    bottom: 14.0,
                    left: 0.0,
                }),
        )
        .height(Length::Fill)
        .on_scroll(|_| Message::Scrolled(ScrollArea::Panel))
        .style(theme::scrollable_auto(
            app.scroll_opacity(ScrollArea::Panel),
        )),
    )
    .width(Length::Fixed(320.0))
    .height(Length::Fill)
    .padding(Padding::from(16))
    .style(theme::settings_panel);

    // 点击面板以外任意位置收回设置；面板内部用 opaque 拦住事件，避免误关。
    let backdrop = mouse_area(
        container(row![])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::veil(0.18)),
    )
    .on_press(Message::ToggleSettings);

    iced::widget::stack![
        backdrop,
        container(row![horizontal_space(), iced::widget::opaque(card)])
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(Padding::from(18)),
    ]
    .into()
}

fn section<'a>(label: &'static str) -> Element<'a> {
    row![
        container(row![])
            .width(3)
            .height(10)
            .style(|_| container::Style {
                background: Some(theme::button_gradient()),
                border: border::rounded(2.0),
                ..container::Style::default()
            }),
        text(label).size(10).color(theme::DIM2),
    ]
    .spacing(7)
    .align_y(Alignment::Center)
    .padding(Padding::from([7, 0]))
    .into()
}

/// 音质胶囊：点击打开选择弹窗。
fn quality_pill<'a>(app: &'a App, dark: bool) -> Element<'a> {
    let label: Element<'a> = if dark {
        widgets::brand_pill(app.player.quality.label())
    } else {
        widgets::pill(app.player.quality.label(), widgets::PillKind::Muted)
    };

    mouse_area(label)
        .on_press(Message::ToggleQualityMenu)
        .interaction(mouse::Interaction::Pointer)
        .into()
}

/// 居中显示的「播放音质」弹窗。
pub(crate) fn quality_dialog(app: &App) -> Element<'_> {
    let mut options = column![].spacing(6).width(Length::Fill);
    for quality in AudioQuality::ALL {
        options = options.push(quality_option(quality, app.player.quality == quality));
    }

    let body = column![
        row![
            text("播放音质").size(16).color(theme::FG),
            horizontal_space(),
            mouse_area(text("关闭").size(11).color(theme::DIM))
                .on_press(Message::ToggleQualityMenu)
                .interaction(mouse::Interaction::Pointer),
        ]
        .align_y(Alignment::Center),
        text("切换后会按当前进度重新加载这首歌")
            .size(11)
            .color(theme::DIM2),
        widgets::gradient_rule(2.0),
        options,
        container(widgets::hint(quality_status(app)))
            .width(Length::Fill)
            .padding(Padding::from([6, 2])),
    ]
    .spacing(12)
    .width(Length::Fill);

    let card = container(body)
        .width(Length::Fixed(360.0))
        .padding(20)
        .style(theme::dialog_panel);

    let backdrop = mouse_area(
        container(row![])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::veil(0.26)),
    )
    .on_press(Message::ToggleQualityMenu);

    iced::widget::stack![
        backdrop,
        container(iced::widget::opaque(card))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill),
    ]
    .into()
}

fn quality_option<'a>(quality: AudioQuality, selected: bool) -> Element<'a> {
    let text_color = if selected { theme::ACCENT } else { theme::FG };

    let marker: Element<'a> = if selected {
        text("当前").size(10).color(theme::DIM2).into()
    } else {
        row![].into()
    };

    button(
        row![
            widgets::dot(if selected { theme::ACCENT } else { theme::LINE }, 6.0,),
            column![
                text(quality.label()).size(13).color(text_color),
                text(quality.detail()).size(10).color(theme::DIM2),
            ]
            .spacing(1),
            horizontal_space(),
            marker,
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([8, 10]))
    .on_press(Message::SetQuality(quality))
    .style(move |_, status| button::Style {
        background: Some(Background::Color(match (selected, status) {
            (true, _) => Color::from_rgb(0.925, 0.945, 1.0),
            (false, button::Status::Hovered) => theme::HOVER_BG,
            _ => theme::PANEL,
        })),
        border: border::rounded(10.0).width(1.0).color(if selected {
            theme::ACCENT
        } else {
            theme::LINE_SOFT
        }),
        text_color,
        ..button::Style::default()
    })
    .into()
}

/// 弹窗底部的一行状态：实际拿到的音源 / VIP 说明。
fn quality_status(app: &App) -> String {
    let loaded = app.player.loaded_quality.as_str();
    if loaded.is_empty() {
        "无损与 Hi-Res 需要黑胶 VIP，实际音源以接口返回为准".to_string()
    } else if loaded == app.player.quality.label() {
        format!("当前音源：{loaded}")
    } else {
        format!(
            "当前音源：{loaded}（{} 不可用，已自动降级）",
            app.player.quality.label()
        )
    }
}

/// 接口把请求档位降级时补一个「实际」提示；没降级就不占位置。
fn quality_note<'a>(app: &'a App, dark: bool) -> Element<'a> {
    let loaded = app.player.loaded_quality.as_str();
    if loaded.is_empty() || loaded == app.player.quality.label() {
        row![].into()
    } else if dark {
        widgets::brand_pill(format!("实际 {loaded}"))
    } else {
        widgets::hint(format!("实际 {loaded}"))
    }
}

fn slider_row<'a>(app: &'a App, label: &'static str, field: VisualField) -> Element<'a> {
    column![
        row![
            text(label).size(12).color(theme::FG),
            horizontal_space(),
            text(app.visual.display(field)).size(11).color(theme::DIM2),
        ]
        .align_y(Alignment::Center),
        seek_bar(
            app.visual.normalized(field),
            SeekStyle {
                height: 20.0,
                thickness: 4.0,
                knob: 5.0,
                colors: BarColors::light(),
            },
            None,
            None,
            move |value| Message::VisualValue(field, value),
        ),
    ]
    .spacing(1)
    .width(Length::Fill)
    .into()
}

fn toggle_row<'a>(label: &'static str, toggle: VisualToggle, on: bool) -> Element<'a> {
    row![
        text(label).size(12).color(theme::FG),
        horizontal_space(),
        button(text(if on { "开" } else { "关" }).size(11).color(if on {
            Color::WHITE
        } else {
            theme::FG
        }))
        .on_press(Message::VisualToggle(toggle))
        .padding(Padding::from([3, 14]))
        .style(theme::toggle(on)),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn palette_row(app: &App) -> Element<'_> {
    let mut grid = column![].spacing(8).width(Length::Fill);

    for chunk in Palette::ALL.chunks(4) {
        let mut row = row![].spacing(8).width(Length::Fill);
        for &palette in chunk {
            row = row.push(palette_swatch(palette, app.visual.palette == palette));
        }
        // 最后一行不够 4 个时用空容器撑住列宽。
        for _ in chunk.len()..4 {
            row = row.push(container(row![]).width(Length::Fill));
        }
        grid = grid.push(row);
    }

    grid.into()
}

fn palette_swatch<'a>(palette: Palette, selected: bool) -> Element<'a> {
    let colors = palette.colors();

    let swatch = container(
        column![
            container(row![])
                .width(Length::Fill)
                .height(16)
                .style(move |_: &iced::Theme| container::Style {
                    background: Some(theme::linear(
                        120.0,
                        &[
                            (0.0, rgb(colors[0])),
                            (0.5, rgb(colors[1])),
                            (1.0, rgb(colors[2])),
                        ],
                    )),
                    border: border::rounded(5.0),
                    ..container::Style::default()
                }),
            text(palette.label())
                .size(9)
                .color(if selected { theme::ACCENT } else { theme::DIM }),
        ]
        .spacing(4)
        .align_x(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([6, 4]))
    .style(move |_: &iced::Theme| container::Style {
        background: Some(Background::Color(if selected {
            Color::from_rgb(0.925, 0.945, 1.0)
        } else {
            Color::from_rgb(0.957, 0.965, 0.980)
        })),
        border: border::rounded(8.0).width(1.0).color(if selected {
            theme::ACCENT
        } else {
            theme::LINE_SOFT
        }),
        ..container::Style::default()
    });

    mouse_area(swatch)
        .on_press(Message::VisualPalette(palette))
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn rgb(color: [f32; 3]) -> Color {
    Color::from_rgb(color[0], color[1], color[2])
}

fn error_banner<'a>(message: &str) -> Element<'a> {
    let content = text(message.to_string())
        .size(11)
        .color(Color::from_rgb(1.0, 0.78, 0.78));

    container(content)
        .padding(Padding::from([8, 10]))
        .style(|_: &iced::Theme| container::Style {
            background: Some(Background::Color(Color::from_rgb(0.361, 0.094, 0.157))),
            border: border::rounded(8.0)
                .width(1.0)
                .color(Color::from_rgba(1.0, 0.6, 0.6, 0.35)),
            ..container::Style::default()
        })
        .into()
}

/// 封面背后的低频光晕：颜色跟随调色板，亮度随鼓点呼吸。
fn cover_halo<'a>(app: &'a App, cover_size: f32, stage: f32) -> Element<'a> {
    let energy = app.ring.bass().clamp(0.0, 1.0);
    let [r, g, b] = app.visual.palette.color_at(0.5);
    let color = Color::from_rgb(r, g, b);
    let size = (cover_size * 2.1).max(120.0);

    container(widgets::glow(size, color, 0.07 + energy * 0.26))
        .width(Length::Fixed(stage))
        .height(Length::Fixed(stage))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

fn big_cover<'a>(app: &'a App, song: &'a Song, size: f32, note: f32) -> Element<'a> {
    let url = song.cover(600);
    // 圆形封面由后台任务生成；未就绪时先用同尺寸的品牌色圆占位，
    // 封面到达后原地替换，不会看到尺寸跳动。
    let handle = app.circle_covers.get(&url).or_else(|| app.covers.get(&url));

    let inner: Element<'a> = match handle {
        Some(handle) => image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Cover)
            .into(),
        None => {
            // 音符放在固定尺寸的盒子里：外层圆每帧随低频缩放，而 iced 0.13
            // 的文本在每帧都变的布局尺寸下会丢掉字形。
            let note_box = note * 2.0;
            container(
                container(text("♪").size(note).color(theme::WHITE_82))
                    .width(Length::Fixed(note_box))
                    .height(Length::Fixed(note_box))
                    .align_x(Alignment::Center)
                    .align_y(Alignment::Center),
            )
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
    };

    container(inner)
        .width(size)
        .height(size)
        .style(|_| container::Style::default())
        .into()
}

// ==========================================================================
// 小工具
// ==========================================================================

/// 音量控制：点击图标静音 / 恢复；悬停图标用滚轮调节，旁边显示当前音量。
fn volume_control<'a>(app: &'a App, size: f32, color: Color) -> Element<'a> {
    let muted = app.player.is_muted();
    let icon = if muted { ICON_VOLUME_MUTE } else { ICON_VOLUME };
    let label = if muted {
        "静音".to_string()
    } else {
        format!("{}%", (app.player.volume * 100.0).round() as i32)
    };

    mouse_area(
        row![
            widgets::svg_icon(icon, size, color),
            text(label).size(10).color(color),
        ]
        .spacing(8)
        .align_y(Alignment::Center)
        .padding(Padding::from([4, 6])),
    )
    .on_press(Message::ToggleMute)
    .on_scroll(Message::VolumeScrolled)
    .interaction(mouse::Interaction::Pointer)
    .into()
}

/// 模式开关按钮：`active` 时用主题蓝，否则用 `idle` 色。
fn mode_icon_button(
    icon: &'static [u8],
    active: bool,
    size: f32,
    idle: Color,
    message: Message,
) -> Element<'static> {
    let color = if active { theme::ACCENT } else { idle };

    button(widgets::svg_icon(icon, size, color))
        .on_press(message)
        .padding(Padding::from([8, 8]))
        .style(theme::player_icon)
        .into()
}

fn icon_button(
    icon: &'static [u8],
    size: f32,
    color: Color,
    message: Message,
    big: bool,
) -> Element<'static> {
    button(widgets::svg_icon(icon, size, color))
        .on_press(message)
        .padding(if big {
            Padding::from([14, 14])
        } else {
            Padding::from([8, 8])
        })
        .style(if big {
            theme::player_play
        } else {
            theme::player_icon
        })
        .into()
}

fn format_time(duration: std::time::Duration) -> String {
    let total = duration.as_secs();
    format!("{}:{:02}", total / 60, total % 60)
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
    use crate::visualizer::VisualizerConfig;

    /// 默认配置下封面半径占画布边长的比例。
    fn cover_ratio() -> f32 {
        crate::viz::center_ring_radius(1.0, &VisualizerConfig::default(), 6)
    }

    #[test]
    fn tall_pages_let_the_ring_fill_the_page() {
        // 加入歌词区后信息栈更高，需要更长的窗口才能让环铺满页面。
        let (size, overlay) = stage_layout(920.0, 1030.0, cover_ratio());
        assert_eq!(size, 920.0);
        assert!(overlay);
    }

    #[test]
    fn short_pages_keep_the_cover_above_the_info() {
        let ratio = cover_ratio();
        let (size, overlay) = stage_layout(1060.0, 660.0, ratio);
        assert!(!overlay);
        let cover_bottom = size * 0.5 + ratio * size;
        assert!(cover_bottom <= 660.0 - INFO_STACK + 0.001);
    }

    #[test]
    fn canvas_keeps_growing_with_the_window() {
        let (small, _) = stage_layout(920.0, 948.0, cover_ratio());
        let (large, _) = stage_layout(1600.0, 1200.0, cover_ratio());
        assert_eq!(large, 1200.0, "不应再撞到 600px 上限");
        assert!(large > small);
    }

    #[test]
    fn stage_metrics_center_matches_the_layout() {
        let config = VisualizerConfig::default();
        let tall = stage_metrics(Size::new(980.0, 1028.0), &config);
        assert!(tall.overlay);
        assert_eq!(tall.center, Point::new(490.0, 514.0));

        let short = stage_metrics(Size::new(1060.0, 660.0), &config);
        assert!(!short.overlay);
        assert_eq!(short.center.x, 530.0);
        assert!((short.center.y - (18.0 + short.size * 0.5)).abs() < 0.001);
    }

    #[test]
    fn dock_hover_zone_is_bottom_center_only() {
        let viewport = Size::new(1000.0, 900.0);
        // 底部中央：展开。
        assert!(pointer_hovers_dock(Point::new(500.0, 880.0), viewport));
        // 底部左边 / 右边：不展开。
        assert!(!pointer_hovers_dock(Point::new(20.0, 880.0), viewport));
        assert!(!pointer_hovers_dock(Point::new(980.0, 880.0), viewport));
        // 窗口中部 / 顶部中央：不展开。
        assert!(!pointer_hovers_dock(Point::new(500.0, 400.0), viewport));
        assert!(!pointer_hovers_dock(Point::new(500.0, 20.0), viewport));
    }

    #[test]
    fn dock_easing_hits_endpoints_and_moves_forward() {
        assert_eq!(ease_in_out(0.0), 0.0);
        assert_eq!(ease_in_out(1.0), 1.0);
        let mut previous = -1.0;
        for step in 0..=10 {
            let value = ease_in_out(step as f32 / 10.0);
            assert!(value >= previous, "缓动不能倒退：{value} < {previous}");
            previous = value;
        }
    }
}

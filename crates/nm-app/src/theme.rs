use iced::gradient::Linear;
use iced::theme::Palette;
use iced::widget::{button, container, scrollable, text_input};
use iced::{border, Background, Border, Color, Radians, Shadow, Theme};

// 色板沿用 KamiDreamML 的浅色风格：蓝灰底 + 白色卡片 + 单一强调蓝。
pub const BG: Color = Color::from_rgb(0.949, 0.957, 0.973); // #F2F4F8
pub const PANEL: Color = Color::WHITE;
pub const LINE: Color = Color::from_rgb(0.878, 0.878, 0.878); // #E0E0E0
pub const LINE_SOFT: Color = Color::from_rgb(0.902, 0.922, 0.949); // #E6EBF2
pub const FG: Color = Color::from_rgb(0.173, 0.243, 0.314); // #2C3E50
pub const DIM: Color = Color::from_rgb(0.565, 0.576, 0.600); // #909399
pub const DIM2: Color = Color::from_rgb(0.690, 0.690, 0.690); // #B0B0B0
pub const ACCENT: Color = Color::from_rgb(0.231, 0.447, 1.0); // #3B72FF
pub const ACCENT_DARK: Color = Color::from_rgb(0.157, 0.322, 0.455); // #285274
pub const HOVER_BG: Color = Color::from_rgb(0.949, 0.965, 1.0); // #F2F6FF
pub const OK: Color = Color::from_rgb(0.102, 0.498, 0.294); // #1A7F4B
pub const WARN: Color = Color::from_rgb(0.706, 0.325, 0.035); // #B45309
pub const ERR: Color = Color::from_rgb(0.878, 0.192, 0.192); // #E03131
/// 红心收藏的强调色：网易云的“喜欢”。
pub const LIKE: Color = Color::from_rgb(0.941, 0.235, 0.337); // #F03C56
pub const VIOLET: Color = Color::from_rgb(0.541, 0.416, 1.0); // #8A6AFF
pub const CYAN: Color = Color::from_rgb(0.408, 0.839, 1.0); // #68D6FF
pub const WHITE_70: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.70);
pub const WHITE_82: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.82);

/// 浅色面板上的「凹陷」底：关闭态开关、次要按钮。
pub const SUNK: Color = Color::from_rgb(0.918, 0.929, 0.953); // #EAEDF3
/// 正在播放页的氛围底色：封面背后那层深靛蓝。
pub const NIGHT: Color = Color::from_rgb(0.075, 0.090, 0.200); // #131726

pub const TRANSPARENT: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 0.0,
};

/// 构造线性渐变背景。
pub fn linear(angle_degrees: f32, stops: &[(f32, Color)]) -> Background {
    let mut gradient = Linear::new(Radians::from(angle_degrees.to_radians()));
    for &(offset, color) in stops {
        gradient = gradient.add_stop(offset, color);
    }
    Background::from(gradient)
}

/// 品牌色块（平铺）：不再使用渐变。
pub fn brand_gradient() -> Background {
    Background::Color(ACCENT)
}

/// 品牌色块（关于页头卡）。
pub fn hero_gradient() -> Background {
    Background::Color(Color::from_rgb(0.184, 0.357, 1.0))
}

/// 按钮色块（平铺）。
pub fn button_gradient() -> Background {
    Background::Color(ACCENT)
}

pub fn app_theme() -> Theme {
    Theme::custom(
        "Montis".to_string(),
        Palette {
            background: BG,
            text: FG,
            primary: ACCENT,
            success: OK,
            danger: ERR,
        },
    )
}

/// 整个窗口的底色：单色平铺。
pub fn page(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(BG)),
        border: Border::default(),
        shadow: Shadow::default(),
    }
}

/// 页面切换时的淡入遮罩：与页面底色同色，`alpha` 由 1 降到 0。
pub fn page_fade(alpha: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: alpha, ..BG })),
        ..container::Style::default()
    }
}

/// 切到「正在播放」页时的深色淡入遮罩。
pub fn dark_fade(alpha: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: alpha, ..NIGHT })),
        ..container::Style::default()
    }
}

/// 「正在播放」页的深色遮罩：让封面变成氛围背景。
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(Color::WHITE),
        background: Some(Background::Color(Color { a: 0.93, ..NIGHT })),
        border: Border::default(),
        shadow: Shadow::default(),
    }
}

/// 登录页左侧品牌卡：纯品牌色块。
pub fn brand_card(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(Color::WHITE),
        background: Some(hero_gradient()),
        border: border::rounded(18.0),
        shadow: Shadow::default(),
    }
}

pub fn card(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(PANEL)),
        border: border::rounded(14.0).width(1.0).color(LINE_SOFT),
        shadow: Shadow::default(),
    }
}

/// 歌单卡片：白底，悬停时铺一层淡蓝并加深描边。
pub fn playlist_card(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);

    button::Style {
        background: Some(Background::Color(if hovered { HOVER_BG } else { PANEL })),
        text_color: FG,
        border: border::rounded(14.0).width(1.0).color(if hovered {
            Color::from_rgba(0.231, 0.447, 1.0, 0.34)
        } else {
            LINE_SOFT
        }),
        shadow: Shadow::default(),
    }
}

/// 浅色页面上的特征卡：纯淡蓝底 + 品牌描边。
pub fn feature_card(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(Color::from_rgb(0.925, 0.945, 1.0))),
        border: border::rounded(16.0)
            .width(1.0)
            .color(Color::from_rgba(0.231, 0.447, 1.0, 0.16)),
        shadow: Shadow::default(),
    }
}

/// 数据卡里的小块：浅蓝平铺底，靠间距形成分栏。
pub fn metric_tile(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(Color::from_rgb(0.953, 0.965, 1.0))),
        border: border::rounded(11.0)
            .width(1.0)
            .color(Color::from_rgba(0.231, 0.447, 1.0, 0.10)),
        shadow: Shadow::default(),
    }
}

/// 侧边栏顶部账号卡：纯白平铺。
pub fn user_tile(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(PANEL)),
        border: border::rounded(12.0).width(1.0).color(LINE_SOFT),
        shadow: Shadow::default(),
    }
}

/// 「正在播放」页的控件台：半透明白底，`alpha` 由设置页控制。
pub fn dock(alpha: f32) -> impl Fn(&Theme) -> container::Style {
    let alpha = alpha.clamp(0.2, 1.0);
    move |_theme: &Theme| container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(Color {
            a: alpha,
            ..Color::WHITE
        })),
        border: border::rounded(22.0)
            .width(1.0)
            .color(Color::from_rgba(1.0, 1.0, 1.0, 0.40)),
        shadow: Shadow::default(),
    }
}

pub fn sidebar(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(PANEL)),
        border: Border {
            color: LINE_SOFT,
            width: 1.0,
            radius: border::Radius::from(0.0),
        },
        shadow: Shadow::default(),
    }
}

pub fn primary(_theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Disabled => Color::from_rgb(0.66, 0.74, 0.95),
        button::Status::Hovered | button::Status::Pressed => Color::from_rgb(0.184, 0.376, 0.925),
        _ => ACCENT,
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: Color::WHITE,
        border: border::rounded(8.0),
        shadow: Shadow::default(),
    }
}

/// 危险操作按钮（清空本地音乐等）：红色实心。
pub fn danger(_theme: &Theme, status: button::Status) -> button::Style {
    let background = match status {
        button::Status::Disabled => Color::from_rgb(0.94, 0.75, 0.75),
        button::Status::Hovered | button::Status::Pressed => Color::from_rgb(0.78, 0.14, 0.14),
        _ => ERR,
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: Color::WHITE,
        border: border::rounded(8.0),
        shadow: Shadow::default(),
    }
}

pub fn ghost(_theme: &Theme, status: button::Status) -> button::Style {
    let (background, border_color, text) = match status {
        button::Status::Hovered | button::Status::Pressed => (HOVER_BG, ACCENT, ACCENT),
        button::Status::Disabled => (PANEL, LINE_SOFT, DIM2),
        _ => (PANEL, LINE, FG),
    };
    button::Style {
        background: Some(Background::Color(background)),
        text_color: text,
        border: border::rounded(7.0).width(1.0).color(border_color),
        shadow: Shadow::default(),
    }
}

/// 浅色卡片上的强调按钮：开启时纯品牌色块，关闭时淡蓝描边。
pub fn soft_accent_button(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    move |_theme: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        if on {
            button::Style {
                background: Some(Background::Color(if hovered {
                    Color::from_rgb(0.184, 0.376, 0.925)
                } else {
                    ACCENT
                })),
                text_color: Color::WHITE,
                border: border::rounded(8.0),
                shadow: Shadow::default(),
            }
        } else {
            button::Style {
                background: Some(Background::Color(if hovered {
                    Color::from_rgb(0.878, 0.906, 1.0)
                } else {
                    Color::from_rgb(0.925, 0.945, 1.0)
                })),
                text_color: ACCENT_DARK,
                border: border::rounded(8.0)
                    .width(1.0)
                    .color(Color::from_rgba(0.231, 0.447, 1.0, 0.22)),
                shadow: Shadow::default(),
            }
        }
    }
}

pub fn icon(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered {
            HOVER_BG
        } else {
            TRANSPARENT
        })),
        text_color: DIM,
        border: border::rounded(6.0),
        shadow: Shadow::default(),
    }
}

/// 设置页的速度档位胶囊。
pub fn speed_chip(selected: bool, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);

    if selected {
        button::Style {
            background: Some(Background::Color(if hovered {
                Color::from_rgb(0.184, 0.376, 0.925)
            } else {
                ACCENT
            })),
            text_color: Color::WHITE,
            border: border::rounded(999.0),
            shadow: Shadow::default(),
        }
    } else {
        button::Style {
            background: Some(Background::Color(if hovered {
                Color::from_rgb(0.878, 0.906, 1.0)
            } else {
                Color::from_rgb(0.949, 0.961, 1.0)
            })),
            text_color: FG,
            border: border::rounded(999.0).width(1.0).color(Color::from_rgba(
                0.231,
                0.447,
                1.0,
                if hovered { 0.30 } else { 0.14 },
            )),
            shadow: Shadow::default(),
        }
    }
}

/// 带淡蓝底的圆形图标按钮（刷新等小操作）。
pub fn soft_icon(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(Color::from_rgba(
            0.231,
            0.447,
            1.0,
            if hovered { 0.18 } else { 0.09 },
        ))),
        text_color: ACCENT,
        border: border::rounded(999.0).width(1.0).color(Color::from_rgba(
            0.231,
            0.447,
            1.0,
            if hovered { 0.36 } else { 0.20 },
        )),
        shadow: Shadow::default(),
    }
}

pub fn window_close(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered {
            Color::from_rgb(1.0, 0.937, 0.937)
        } else {
            TRANSPARENT
        })),
        text_color: if hovered { ERR } else { DIM },
        border: border::rounded(6.0),
        shadow: Shadow::default(),
    }
}

/// 深色页面上的窗口按钮（最小化）。
pub fn icon_dark(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered {
            Color::from_rgba(1.0, 1.0, 1.0, 0.12)
        } else {
            TRANSPARENT
        })),
        text_color: WHITE_70,
        border: border::rounded(6.0),
        shadow: Shadow::default(),
    }
}

/// 深色页面上的关闭按钮。
pub fn window_close_dark(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered {
            Color::from_rgba(1.0, 0.52, 0.52, 0.18)
        } else {
            TRANSPARENT
        })),
        text_color: if hovered {
            Color::from_rgb(1.0, 0.62, 0.62)
        } else {
            WHITE_70
        },
        border: border::rounded(6.0),
        shadow: Shadow::default(),
    }
}

pub fn input(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(
        status,
        text_input::Status::Focused | text_input::Status::Hovered
    );
    text_input::Style {
        background: Background::Color(PANEL),
        border: border::rounded(7.0)
            .width(1.0)
            .color(if focused { ACCENT } else { LINE }),
        icon: DIM,
        placeholder: DIM2,
        value: FG,
        selection: Color::from_rgba(0.231, 0.447, 1.0, 0.25),
    }
}

pub fn scrollable_style(_theme: &Theme, _status: scrollable::Status) -> scrollable::Style {
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            color: Color::from_rgba(0.55, 0.60, 0.70, 0.55),
            border: border::rounded(4.0),
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
    }
}

/// 按需显示的滚动条：`fade` 是当前不透明度（0 = 隐藏）。
///
/// 是否需要显示由调用方根据「最近一次滚动时间」决定，静止 0.6 秒后淡出。
pub fn scrollable_auto(fade: f32) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style {
    move |theme, status| {
        let fade = fade.clamp(0.0, 1.0);
        // 完全透明时直接用隐藏版，避免每帧画一块看不见的滑块。
        if fade <= 0.004 {
            return scrollable_hidden(theme, status);
        }

        // 对基础样式做透明度混合，出现 / 消失都是渐变而不是硬切。
        let mut style = scrollable_style(theme, status);
        style.vertical_rail.scroller.color.a *= fade;
        style.horizontal_rail.scroller.color.a *= fade;
        style
    }
}

/// 隐藏滚动条但保留滚轮 / 触控板滚动，用于首页这种整页滚动的区域。
pub fn scrollable_hidden(_theme: &Theme, _status: scrollable::Status) -> scrollable::Style {
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            color: TRANSPARENT,
            border: border::rounded(0.0),
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
    }
}

/// 首页底部播放条：纯白平铺，用一条顶部细线分界。
pub fn player_bar(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(PANEL)),
        border: Border {
            color: LINE_SOFT,
            width: 1.0,
            radius: border::Radius::from(0.0),
        },
        shadow: Shadow::default(),
    }
}

/// 播放控件的小圆按钮（透明底，悬停时提亮）。
/// 可点击列表行的悬停底色。
/// 歌曲行：当前播放的一行始终带一层淡蓝底，悬停时再提亮。
pub fn row_active(active: bool) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    move |_theme: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(if active {
                if hovered {
                    Color::from_rgb(0.878, 0.910, 1.0)
                } else {
                    Color::from_rgb(0.914, 0.937, 1.0)
                }
            } else if hovered {
                HOVER_BG
            } else {
                TRANSPARENT
            })),
            text_color: FG,
            border: border::rounded(8.0),
            shadow: Shadow::default(),
        }
    }
}

pub fn player_icon(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered {
            HOVER_BG
        } else {
            TRANSPARENT
        })),
        text_color: FG,
        border: border::rounded(999.0),
        shadow: Shadow::default(),
    }
}

/// 深色背景上的胶囊按钮：平时透明，悬停铺一层白底。
/// 用于「返回」这类带文字的轻量操作，命中区比裸 `mouse_area` 更大。
pub fn ghost_pill(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered {
            Color::from_rgba(1.0, 1.0, 1.0, 0.12)
        } else {
            TRANSPARENT
        })),
        text_color: Color::WHITE,
        border: border::rounded(999.0),
        shadow: Shadow::default(),
    }
}

/// 主播放键：品牌蓝圆盘，白图标。
pub fn player_play(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered {
            Color::from_rgb(0.184, 0.376, 0.925)
        } else {
            ACCENT
        })),
        text_color: Color::WHITE,
        border: border::rounded(999.0),
        shadow: Shadow::default(),
    }
}

/// 可视化设置面板：白底卡片。
pub fn settings_panel(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(PANEL)),
        border: border::rounded(16.0).width(1.0).color(LINE_SOFT),
        shadow: Shadow::default(),
    }
}

/// 居中弹窗卡：白底卡片。
pub fn dialog_panel(_theme: &Theme) -> container::Style {
    container::Style {
        text_color: Some(FG),
        background: Some(Background::Color(PANEL)),
        border: border::rounded(16.0).width(1.0).color(LINE_SOFT),
        shadow: Shadow::default(),
    }
}

/// 设置面板里的开关按钮。
pub fn toggle(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
    move |_theme: &Theme, status: button::Status| {
        let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
        button::Style {
            background: Some(Background::Color(if on {
                ACCENT
            } else if hovered {
                HOVER_BG
            } else {
                SUNK
            })),
            text_color: if on { Color::WHITE } else { FG },
            border: border::rounded(999.0).width(1.0).color(if on {
                Color::from_rgba(1.0, 1.0, 1.0, 0.18)
            } else {
                LINE
            }),
            shadow: Shadow::default(),
        }
    }
}

/// 浮层背后的一层薄遮罩：用前景色压暗页面，保持浅色调。
pub fn veil(alpha: f32) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: alpha, ..FG })),
        ..container::Style::default()
    }
}

/// 浅色背景上的次级描边按钮（恢复默认等）。
pub fn ghost_soft(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(if hovered { HOVER_BG } else { SUNK })),
        text_color: FG,
        border: border::rounded(8.0).width(1.0).color(LINE),
        shadow: Shadow::default(),
    }
}

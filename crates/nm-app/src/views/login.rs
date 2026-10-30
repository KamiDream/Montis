use iced::widget::{
    button, column, container, horizontal_space, row, text, text_input, vertical_space,
};
use iced::{border, Alignment, Background, Color, Length, Padding};

use crate::app::{App, Element, Message};
use crate::theme;
use crate::widgets::{self, PillKind};

pub fn view(app: &App) -> Element<'_> {
    container(
        row![brand_card(app), login_card(app)]
            .spacing(22)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .padding(28)
    .into()
}

fn brand_card(app: &App) -> Element<'_> {
    let logo_tile = container(widgets::logo(54.0))
        .padding(8)
        .style(|_| container::Style {
            background: Some(Background::Color(Color::WHITE)),
            border: border::rounded(20.0),
            ..container::Style::default()
        });

    let features = column![
        widgets::feature_line_dark("Cookie 登录，不接触账号密码"),
        widgets::feature_line_dark("会话保存在本地，权限 600"),
        widgets::feature_line_dark("为播放与频谱可视化而生"),
    ]
    .spacing(9);

    let badges = row![
        widgets::brand_pill("v0.1.0"),
        widgets::brand_pill("本地优先"),
    ]
    .spacing(6);

    let spectrum_header = row![
        widgets::pulse_dot(Color::from_rgba(1.0, 1.0, 1.0, 0.95), app.phase, 7.0),
        text("VISUAL SPECTRUM").size(9).color(theme::WHITE_70),
        horizontal_space(),
        text("PREVIEW")
            .size(9)
            .color(Color::from_rgba(1.0, 1.0, 1.0, 0.45)),
    ]
    .spacing(7)
    .align_y(Alignment::Center);

    let body = column![
        logo_tile,
        column![
            text("Montis").size(32).color(Color::WHITE),
            text("山音 · 网易云音乐客户端")
                .size(12)
                .color(theme::WHITE_70),
        ]
        .spacing(3),
        container(row![])
            .width(Length::Fill)
            .height(1)
            .style(|_| container::Style {
                background: Some(Background::Color(Color::from_rgba(1.0, 1.0, 1.0, 0.18))),
                ..container::Style::default()
            }),
        features,
        vertical_space(),
        spectrum_header,
        widgets::equalizer(app.phase, 24, 3.5, 44.0, Color::WHITE),
        badges,
    ]
    .spacing(10)
    .width(Length::Fill)
    .height(Length::Fill);

    container(iced::widget::stack![
        container(brand_orbit())
            .width(Length::Fill)
            .height(Length::Fill)
            .align_x(Alignment::End)
            .align_y(Alignment::Start),
        body,
    ])
    .width(Length::Fixed(310.0))
    .height(Length::Fixed(500.0))
    .padding(28)
    .style(theme::brand_card)
    .into()
}

/// 品牌卡右上角的装饰圆环。
fn brand_orbit<'a>() -> Element<'a> {
    let ring = |size: f32, alpha: f32| {
        container(row![])
            .width(size)
            .height(size)
            .style(move |_| container::Style {
                border: border::rounded(size * 0.5)
                    .width(1.0)
                    .color(Color::from_rgba(1.0, 1.0, 1.0, alpha)),
                ..container::Style::default()
            })
    };

    container(iced::widget::stack![
        ring(260.0, 0.055),
        ring(180.0, 0.075),
        ring(104.0, 0.10),
    ])
    .padding(Padding {
        top: 0.0,
        right: 0.0,
        bottom: 0.0,
        left: 0.0,
    })
    .into()
}

fn login_card(app: &App) -> Element<'_> {
    let header = column![
        row![
            column![
                row![
                    widgets::dot(theme::ACCENT, 5.0),
                    text("SIGN IN").size(9).color(theme::DIM2),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                text("连接你的网易云账号").size(22).color(theme::FG),
                text("粘贴浏览器里的 Cookie，无需在客户端输入密码")
                    .size(11)
                    .color(theme::DIM),
            ]
            .spacing(4),
            horizontal_space(),
            widgets::pill("安全登录", PillKind::Ok),
        ]
        .align_y(Alignment::Center),
        widgets::gradient_rule(2.0),
    ]
    .spacing(10);

    container(
        column![header, cookie_panel(app), vertical_space(), guarantees(),]
            .spacing(16)
            .height(Length::Fill),
    )
    .width(Length::Fixed(500.0))
    .height(Length::Fixed(500.0))
    .padding(26)
    .style(theme::card)
    .into()
}

/// 登录卡底部的本地隐私说明。
fn guarantees<'a>() -> Element<'a> {
    let tile = |title: &'a str, detail: &'a str, color: Color| {
        container(
            column![
                row![
                    widgets::dot(color, 5.0),
                    text(title).size(11).color(theme::FG),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                text(detail).size(9).color(theme::DIM2),
            ]
            .spacing(3),
        )
        .width(Length::Fill)
        .padding(Padding::from([9, 10]))
        .style(theme::metric_tile)
    };

    row![
        tile("仅本机存储", "Cookie 写入本地会话文件", theme::ACCENT),
        tile("不接触密码", "只校验 MUSIC_U 凭证", theme::VIOLET),
        tile("会话可撤回", "退出登录即清空凭证", theme::OK),
    ]
    .spacing(8)
    .into()
}

fn cookie_panel(app: &App) -> Element<'_> {
    let steps = container(
        column![
            row![
                text("获取 Cookie").size(10).color(theme::ACCENT_DARK),
                horizontal_space(),
                widgets::pill("3 步", PillKind::Muted),
            ]
            .align_y(Alignment::Center),
            guide_row("1", "在浏览器登录 music.163.com"),
            guide_row("2", "开发者工具 → Application → Cookies"),
            guide_row("3", "复制 MUSIC_U，或点「粘贴」导入整段 Cookie"),
        ]
        .spacing(7),
    )
    .width(Length::Fill)
    .padding(Padding::from([12, 14]))
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.961, 0.973, 1.0))),
        border: border::rounded(11.0)
            .width(1.0)
            .color(Color::from_rgba(0.231, 0.447, 1.0, 0.14)),
        ..container::Style::default()
    });

    let input = text_input("MUSIC_U=...; __csrf=...", app.cookie_input.as_str())
        .on_input(Message::CookieChanged)
        .on_submit(Message::CookieSubmit)
        .padding(11)
        .size(12)
        .width(Length::Fill)
        .style(theme::input);

    // 输入框粘贴时会丢掉换行与制表符，从 Cookies 表格整行复制的内容会
    // 被拼坏；这个按钮直接读剪贴板原始内容，再做归一化。
    let paste = button(text("粘贴").size(11).color(theme::FG))
        .on_press(Message::PasteCookie)
        .padding(Padding::from([11, 14]))
        .style(theme::ghost);

    let input = row![input, paste].spacing(8).align_y(Alignment::Center);

    let action: Element<'_> = if app.cookie_busy {
        button(
            row![
                widgets::spinner(app.phase, 2.5, iced::Color::WHITE),
                text("正在校验 Cookie…").size(13).color(iced::Color::WHITE),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([10, 16]))
        .style(theme::primary)
        .into()
    } else {
        button(
            row![
                widgets::svg_icon(widgets::ICON_COOKIE, 13.0, iced::Color::WHITE),
                text("使用 Cookie 登录").size(13).color(iced::Color::WHITE),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
        )
        .on_press(Message::CookieSubmit)
        .padding(Padding::from([10, 16]))
        .style(theme::primary)
        .into()
    };

    let action = container(action)
        .width(Length::Fill)
        .align_x(Alignment::Center);

    let footer: Element<'_> = match &app.login_error {
        Some(error) => widgets::error_text(error.clone()),
        None => widgets::hint("MUSIC_U 等同于登录凭证，只会保存在本机。"),
    };

    column![steps, input, action, footer].spacing(12).into()
}

fn guide_row<'a>(index: &'a str, value: &'a str) -> Element<'a> {
    row![
        container(text(index).size(10).color(Color::WHITE))
            .width(18)
            .height(18)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(|_| container::Style {
                background: Some(theme::button_gradient()),
                border: border::rounded(9.0),
                ..container::Style::default()
            }),
        text(value).size(12).color(theme::FG),
    ]
    .spacing(9)
    .align_y(Alignment::Center)
    .into()
}

use iced::widget::{
    button, column, container, horizontal_space, row, text, text_input, vertical_space,
};
use iced::{border, Alignment, Background, Color, Length, Padding};

use crate::app::{App, Element, Message};
use crate::theme;
use crate::widgets::{self, PillKind};

/// 哔哩哔哩品牌粉：步骤序号、提示卡描边都用它。
fn bili_pink() -> Color {
    Color::from_rgb(0.976, 0.376, 0.545)
}

/// 网易云登录卡片：只处理网易云。
pub(crate) fn netease_card(app: &App) -> Element<'_> {
    card(
        "连接你的网易云账号",
        "粘贴浏览器里的 Cookie，无需在客户端输入密码",
        "只校验 MUSIC_U 凭证",
        theme::ACCENT,
        cookie_panel(app),
    )
}

/// 哔哩哔哩登录卡片：与网易云同一套布局，表单与配色换成自己的。
pub(crate) fn bili_card(app: &App) -> Element<'_> {
    card(
        "连接你的哔哩哔哩账号",
        "只登录收藏夹也能使用，播放时不会打开视频",
        "只校验 SESSDATA 凭证",
        bili_pink(),
        bili_cookie_panel(app),
    )
}

/// 两个渠道共用的登录卡外壳：标题、分割线、表单与底部隐私说明。
fn card<'a>(
    title: &'a str,
    subtitle: &'a str,
    credentials: &'static str,
    accent: Color,
    panel: Element<'a>,
) -> Element<'a> {
    let header = column![
        row![
            column![
                row![
                    widgets::dot(accent, 5.0),
                    text("SIGN IN").size(9).color(theme::DIM2),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
                text(title).size(22).color(theme::FG),
                text(subtitle).size(11).color(theme::DIM),
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
        column![header, panel, vertical_space(), guarantees(credentials)]
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
fn guarantees(credentials: &'static str) -> Element<'static> {
    let tile = |title: &'static str, detail: &'static str, color: Color| {
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
        tile("不接触密码", credentials, theme::VIOLET),
        tile("会话可撤回", "退出登录即清空凭证", theme::OK),
    ]
    .spacing(8)
    .into()
}

/// 网易云表单：获取 MUSIC_U 的步骤 + 输入框 + 登录按钮。
fn cookie_panel(app: &App) -> Element<'_> {
    let steps = container(
        column![
            row![
                text("获取 Cookie").size(10).color(theme::ACCENT_DARK),
                horizontal_space(),
                widgets::pill("3 步", PillKind::Muted),
            ]
            .align_y(Alignment::Center),
            step_row("1", "在浏览器登录 music.163.com", theme::button_gradient()),
            step_row(
                "2",
                "开发者工具 → Application → Cookies",
                theme::button_gradient()
            ),
            step_row(
                "3",
                "复制 MUSIC_U，或点「粘贴」导入整段 Cookie",
                theme::button_gradient()
            ),
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

/// 哔哩哔哩表单：获取 SESSDATA 的步骤 + 输入框 + 连接按钮。
fn bili_cookie_panel(app: &App) -> Element<'_> {
    let steps = container(
        column![
            row![
                text("获取 Cookie").size(10).color(theme::ACCENT_DARK),
                horizontal_space(),
                widgets::pill("仅需 SESSDATA", PillKind::Muted),
            ]
            .align_y(Alignment::Center),
            step_row(
                "1",
                "在浏览器登录 bilibili.com",
                Background::Color(bili_pink())
            ),
            step_row(
                "2",
                "开发者工具 → Application → Cookies",
                Background::Color(bili_pink())
            ),
            step_row(
                "3",
                "复制 SESSDATA，或点「粘贴」导入整段 Cookie",
                Background::Color(bili_pink())
            ),
        ]
        .spacing(7),
    )
    .width(Length::Fill)
    .padding(Padding::from([12, 14]))
    .style(|_| container::Style {
        background: Some(Background::Color(Color::from_rgb(0.996, 0.961, 0.973))),
        border: border::rounded(11.0)
            .width(1.0)
            .color(Color::from_rgba(0.976, 0.376, 0.545, 0.18)),
        ..container::Style::default()
    });

    let input = text_input(
        "SESSDATA=...; bili_jct=...",
        app.bili_state.cookie_input.as_str(),
    )
    .on_input(Message::BiliCookieChanged)
    .on_submit(Message::BiliConnect)
    .padding(11)
    .size(12)
    .width(Length::Fill)
    .style(theme::input);

    let paste = button(text("粘贴").size(11).color(theme::FG))
        .on_press(Message::BiliPasteCookie)
        .padding(Padding::from([11, 14]))
        .style(theme::ghost);

    let input = row![input, paste].spacing(8).align_y(Alignment::Center);

    let action: Element<'_> = if app.bili_state.connecting {
        button(
            row![
                widgets::spinner(app.phase, 2.5, iced::Color::WHITE),
                text("正在校验 Cookie…").size(13).color(iced::Color::WHITE),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        )
        .padding(Padding::from([10, 16]))
        .style(theme::bili_primary)
        .into()
    } else {
        button(
            row![
                widgets::svg_icon(widgets::ICON_BILIBILI, 13.0, iced::Color::WHITE),
                text("连接哔哩哔哩").size(13).color(iced::Color::WHITE),
            ]
            .spacing(7)
            .align_y(Alignment::Center),
        )
        .on_press(Message::BiliConnect)
        .padding(Padding::from([10, 16]))
        .style(theme::bili_primary)
        .into()
    };

    let action = container(action)
        .width(Length::Fill)
        .align_x(Alignment::Center);

    let footer: Element<'_> = match &app.bili_state.connect_error {
        Some(error) => widgets::error_text(error.clone()),
        None => widgets::hint("SESSDATA 等同于登录凭证，只会保存在本机（权限 600）。"),
    };

    column![steps, input, action, footer].spacing(12).into()
}

/// 步骤序号：白色数字 + 渠道色圆角底。
fn step_row<'a>(index: &'a str, value: &'a str, background: Background) -> Element<'a> {
    row![
        container(text(index).size(10).color(Color::WHITE))
            .width(18)
            .height(18)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(move |_| container::Style {
                background: Some(background),
                border: border::rounded(9.0),
                ..container::Style::default()
            }),
        text(value).size(12).color(theme::FG),
    ]
    .spacing(9)
    .align_y(Alignment::Center)
    .into()
}

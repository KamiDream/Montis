//! 首次启动的使用须知：确认前不进入应用。
//!
//! 确认状态保存在 `settings.json`，之后不再弹出。

use iced::widget::{button, column, container, horizontal_space, mouse_area, row};
use iced::{border, Alignment, Background, Length, Padding};

use crate::app::{Element, Message};
use crate::widgets::text;
use crate::{theme, widgets};

const POINTS: [(&str, &str); 6] = [
    (
        "非官方项目",
        "与网易云音乐及其关联公司没有任何关系，也未获得其授权、认可或赞助。",
    ),
    (
        "仅供学习研究",
        "本项目用于学习 Rust、音频可视化与网络协议实践，严禁任何商业用途。",
    ),
    (
        "接口可能失效",
        "数据来自公开的 Web 端接口，可能随时调整、失效或触发风控；请遵守平台条款与当地法律。",
    ),
    (
        "下载仅限个人",
        "下载功能只用于个人已获授权内容的离线收听，请勿复制、传播、出售或再分发。",
    ),
    (
        "本地保存",
        "登录只使用你自己提供的 Cookie，不接触、不保存密码；会话与偏好只写入本机。",
    ),
    (
        "风险自担",
        "使用本项目产生的账号风控、数据丢失或其他后果，均由使用者自行承担。",
    ),
];

pub fn view() -> Element<'static> {
    let mut points = column![].spacing(10);
    for (index, (title, body)) in POINTS.into_iter().enumerate() {
        points = points.push(point(index + 1, title, body));
    }

    let quit = button(text("退出").size(12).color(theme::DIM))
        .on_press(Message::Close)
        .padding(Padding::from([10, 18]))
        .style(theme::ghost);
    let accept = button(text("我已阅读并同意").size(12).color(iced::Color::WHITE))
        .on_press(Message::AcceptDisclaimer)
        .padding(Padding::from([10, 24]))
        .style(theme::primary);

    let body = column![
        text("MONTIS · 山音").size(9).color(theme::ACCENT_DARK),
        text("使用须知").size(22).color(theme::FG),
        text("开始之前，先叠个甲。确认后即可进入，之后不再提示。")
            .size(11)
            .color(theme::DIM2),
        widgets::gradient_rule(2.0),
        points,
        text("点击「我已阅读并同意」即表示你已理解并接受以上说明。")
            .size(10)
            .color(theme::DIM2),
        row![horizontal_space(), quit, accept]
            .spacing(10)
            .align_y(Alignment::Center),
    ]
    .spacing(14)
    .width(Length::Fill);

    let card = container(body)
        .width(Length::Fixed(560.0))
        .padding(Padding::from([24, 26]))
        .style(theme::dialog_panel);

    // 遮罩吞掉所有点击：确认前不能操作下面的页面。
    let backdrop = mouse_area(
        container(row![])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::veil(0.55)),
    )
    .on_press(Message::DisclaimerBlocked);

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

fn point(index: usize, title: &'static str, body: &'static str) -> Element<'static> {
    row![
        container(
            text(format!("{index:02}"))
                .size(9)
                .color(iced::Color::WHITE)
        )
        .width(Length::Fixed(22.0))
        .height(Length::Fixed(22.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|_| container::Style {
            background: Some(Background::Color(theme::ACCENT)),
            border: border::rounded(7.0),
            ..container::Style::default()
        }),
        column![
            text(title).size(12).color(theme::FG),
            text(body).size(11).color(theme::DIM),
        ]
        .spacing(2)
        .width(Length::Fill),
    ]
    .spacing(10)
    .align_y(Alignment::Start)
    .into()
}

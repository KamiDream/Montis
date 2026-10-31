use iced::widget::{column, container, horizontal_space, row, vertical_space};
use iced::{border, Alignment, Background, Color, Length, Padding};

use crate::app::{App, Element, Message};
use crate::theme;
use crate::widgets::{self, text, PillKind};

pub fn view(app: &App) -> Element<'_> {
    let failed = app.restore_error.is_some();

    let header = row![
        row![
            widgets::dot(theme::ACCENT, 5.0),
            text("MONTIS · 山音").size(9).color(theme::DIM2),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
        horizontal_space(),
        widgets::pill(
            if failed {
                "需要重新登录"
            } else {
                "本地会话"
            },
            if failed {
                PillKind::Warn
            } else {
                PillKind::Muted
            },
        ),
    ]
    .align_y(Alignment::Center);

    let body: Element<'_> = if let Some(error) = &app.restore_error {
        column![
            logo_stage(true),
            text("无法恢复登录状态").size(21).color(theme::FG),
            text(error.clone()).size(11).color(theme::ERR),
            vertical_space().height(2),
            row![
                container(widgets::primary_button("重试", None, Message::RetrySession))
                    .width(Length::Fixed(160.0)),
                container(widgets::ghost_button(
                    "重新登录",
                    None,
                    Message::BackToLogin
                ))
                .width(Length::Fixed(160.0)),
            ]
            .spacing(10),
        ]
        .spacing(12)
        .align_x(Alignment::Center)
        .into()
    } else {
        column![
            logo_stage(false),
            text("正在恢复登录状态").size(21).color(theme::FG),
            text("正在向网易云音乐校验本地会话…")
                .size(12)
                .color(theme::DIM),
            vertical_space().height(4),
            widgets::spinner(app.phase, 7.0, theme::ACCENT),
        ]
        .spacing(12)
        .align_x(Alignment::Center)
        .into()
    };

    let mut card_body = column![
        header,
        widgets::gradient_rule(2.0),
        container(body)
            .width(Length::Fill)
            .align_x(Alignment::Center),
    ]
    .spacing(16);

    card_body = card_body.push(
        container(widgets::hint(if failed {
            "会话文件位于 ~/.config/melody/session.json，也可以直接重新粘贴 Cookie。"
        } else {
            "校验通过后会自动进入首页，不需要再次输入 Cookie。"
        }))
        .width(Length::Fill)
        .align_x(Alignment::Center),
    );

    if let Some(warning) = &app.storage_error {
        card_body = card_body.push(
            row![
                widgets::pill("提示", PillKind::Warn),
                widgets::hint(warning.clone())
            ]
            .spacing(8)
            .align_y(Alignment::Center),
        );
    }

    container(
        container(card_body)
            .width(Length::Fixed(460.0))
            .padding(Padding::from(28))
            .style(theme::card),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .center_x(Length::Fill)
    .center_y(Length::Fill)
    .padding(28)
    .into()
}

/// 居中的品牌标志：白色圆角底。
fn logo_stage<'a>(failed: bool) -> Element<'a> {
    let color = if failed { theme::ERR } else { theme::ACCENT };

    let tile = container(widgets::logo(58.0))
        .padding(9)
        .style(move |_| container::Style {
            text_color: Some(theme::FG),
            background: Some(Background::Color(Color::WHITE)),
            border: border::rounded(20.0).width(1.0).color(color),
            ..container::Style::default()
        });

    container(tile)
        .width(Length::Fixed(120.0))
        .height(Length::Fixed(120.0))
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .into()
}

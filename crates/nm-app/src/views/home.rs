use iced::widget::{
    button, column, container, horizontal_space, image, mouse_area, row, scrollable, text_input,
    vertical_space,
};
use iced::{border, mouse, Alignment, Background, ContentFit, Length, Padding};

use nm_api::{ArtistDetail, BiliFolder, PlaylistDetail, PlaylistSummary, Song};

use crate::app::{App, Element, LikedTransferKind, Message, PlaylistState, ScrollArea};
use crate::audio_cache::{format_bytes, MB};
use crate::theme;
use crate::ui_settings::ANIMATION_PRESETS;
use crate::views::scroll_window;
use crate::widgets::{self, text, PillKind};

pub fn view(app: &App) -> Element<'_> {
    // 歌单首页 ↔ 歌单详情页切换时，只让右侧内容区淡入，侧边栏保持不动。
    // 遮罩层始终存在、只改透明度：按需包一层 `stack` 会在动画结束时重建整个内容区，
    // 列表的滚动位置等状态会丢掉。
    let fade = app.view_fade.clamp(0.0, 1.0);
    let content: Element<'_> = iced::widget::stack![
        main(app),
        container(row![])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(move |_| theme::page_fade(fade)),
    ]
    .width(Length::Fill)
    .height(Length::Fill)
    .into();

    let body = container(row![sidebar(app), content])
        .width(Length::Fill)
        .height(Length::Fill);

    let mut layout = column![body].width(Length::Fill).height(Length::Fill);

    if app.player.current.is_some() {
        layout = layout.push(crate::views::player::bar(app));
    }

    container(layout)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn sidebar(app: &App) -> Element<'_> {
    // 本地「我喜欢的音乐」是一个独立页面：打开它时不能同时把「我的歌单」点亮。
    let liked_active = app
        .playlist
        .as_ref()
        .map(|state| state.local_liked)
        .unwrap_or(false);
    // 本地音乐（下载）也是独立页面。
    let downloads_active = app
        .playlist
        .as_ref()
        .map(|state| state.local_downloads)
        .unwrap_or(false);

    // 歌单详情是「我的歌单」的子页面：进入详情后选中态保持不动，
    // 侧边栏始终能回答「我在哪一页」，而不是一进详情就整列变灰。
    let discover_active = !liked_active
        && !downloads_active
        && !app.settings_page
        && !app.about_page
        && !app.search_page
        && !app.daily_page
        && !app.fm_page
        && !app.bili_page
        && !app.netease_login_page
        && app.artist.is_none();
    let discover = nav_item(
        "我的歌单",
        "搜索 · 歌单",
        widgets::ICON_HOME,
        discover_active,
        Message::ClosePlaylist,
    );

    let search = nav_item(
        "搜索",
        "歌名 · 歌手 · 专辑",
        widgets::ICON_SEARCH,
        app.search_page,
        Message::OpenSearch,
    );

    let daily = nav_item(
        "每日推荐",
        "按口味每天更新",
        widgets::ICON_DAILY,
        app.daily_page,
        Message::OpenDaily,
    );

    let fm = nav_item(
        "私人FM",
        "按口味持续推荐",
        widgets::ICON_FM,
        app.fm_page,
        Message::OpenFm,
    );

    let bilibili = nav_item(
        "哔哩哔哩",
        "收藏夹 · 纯音频",
        widgets::ICON_BILIBILI,
        app.bili_page,
        Message::OpenBilibili,
    );

    let liked = nav_item(
        "我喜欢的音乐",
        "本地收藏",
        widgets::ICON_HEART,
        liked_active,
        Message::OpenLiked,
    );

    let downloads = nav_item(
        "本地音乐",
        "下载 · 离线播放",
        widgets::ICON_DOWNLOAD,
        downloads_active,
        Message::OpenDownloads,
    );

    // 网易云分组：账号卡片放在分组标题下，歌单与在线推荐都在这组。
    // 进入某张歌单时，在「我的歌单」下方挂一个缩进的子节点，把当前
    // 打开的是哪张歌单也写出来。
    let mut library =
        column![sidebar_label("网易云音乐", None), user_card(app), discover].spacing(8);
    if let Some(name) = current_playlist_name(app) {
        // 我的歌单 / 喜欢的音乐 / 本地音乐 / 哔哩哔哩收藏夹都有自己的入口，
        // 不再重复一层子节点。
        if !liked_active && !downloads_active && !app.bili_page {
            library = library.push(current_playlist_row(&name));
        }
    }
    library = library.push(daily);
    library = library.push(fm);
    library = library.push(search);

    // 哔哩哔哩是独立的「外部渠道」，不跟本地音乐库混在一起，
    // 单独开一个分组，账号与收藏夹入口都在这个分组下。
    // 进入某个收藏夹后，在按钮下面挂一个缩进的子节点，和网易云歌单一致。
    let mut bilibili_group = column![
        sidebar_label("BiliBili", None),
        bili_user_card(app),
        bilibili
    ]
    .spacing(8);
    if let Some(name) = current_bili_folder_name(app) {
        bilibili_group = bilibili_group.push(current_playlist_row(&name));
    }

    // 本地分组：红心收藏与下载都只存在本机，放在最下面一组。
    let local = column![sidebar_label("本地", None), liked, downloads].spacing(8);

    // 分组多了以后在小窗口里可能放不下：导航区可以滚动，
    // 设置 / 关于固定在底部，不会跟着被挤出窗口。
    let navigation = column![
        library,
        widgets::vspace(4.0),
        bilibili_group,
        widgets::vspace(4.0),
        local,
    ]
    .spacing(8)
    .width(Length::Fill);

    container(
        column![
            widgets::vspace(6.0),
            scrollable(navigation)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::scrollable_hidden),
            widgets::separator(),
            settings_card(app),
            about_card(app),
        ]
        .spacing(8)
        .height(Length::Fill),
    )
    .width(Length::Fixed(250.0))
    .height(Length::Fill)
    .padding(Padding::from([16, 14]))
    .style(theme::sidebar)
    .into()
}

/// 当前打开的歌单名：详情还没回来时先从歌单列表里推断，避免加载期间侧栏闪空。
fn current_playlist_name(app: &App) -> Option<String> {
    let state = app.playlist.as_ref()?;
    // 哔哩哔哩收藏夹有自己的子节点，不能混进网易云歌单。
    if state.bilibili {
        return None;
    }
    state
        .detail
        .as_ref()
        .map(|detail| detail.name.clone())
        .or_else(|| {
            app.library
                .playlists
                .iter()
                .find(|item| item.id == state.id)
                .map(|item| item.name.clone())
        })
}

/// 当前打开的哔哩哔哩收藏夹名：详情还没回来时先从收藏夹列表里推断。
fn current_bili_folder_name(app: &App) -> Option<String> {
    let state = app.playlist.as_ref()?;
    if !app.bili_page || !state.bilibili {
        return None;
    }

    state
        .detail
        .as_ref()
        .map(|detail| detail.name.clone())
        .filter(|name| !name.trim().is_empty())
        .or_else(|| {
            app.bili_state
                .folders
                .iter()
                .find(|folder| folder.id == state.id)
                .map(|folder| folder.title.clone())
        })
}

/// 「我的歌单」下的子节点：一根短竖线画出层级缩进，右边是歌单名。
fn current_playlist_row(name: &str) -> Element<'static> {
    container(
        row![
            container(row![])
                .width(2)
                .height(15)
                .style(|_| container::Style {
                    background: Some(Background::Color(theme::ACCENT)),
                    border: border::rounded(1.0),
                    ..container::Style::default()
                }),
            text(clip(name, 16)).size(11).color(theme::ACCENT_DARK),
        ]
        .spacing(9)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([5, 14]))
    .style(|_| container::Style {
        background: Some(Background::Color(iced::Color::from_rgba(
            0.152, 0.353, 1.0, 0.05,
        ))),
        border: border::rounded(8.0),
        ..container::Style::default()
    })
    .into()
}

/// 侧边栏分组标题：一条渐变短竖线 + 小号标签，右侧可挂操作链接。
fn sidebar_label<'a>(label: &'a str, trailing: Option<Element<'a>>) -> Element<'a> {
    let mut content = row![
        container(row![])
            .width(3)
            .height(11)
            .style(|_| container::Style {
                background: Some(theme::button_gradient()),
                border: border::rounded(2.0),
                ..container::Style::default()
            }),
        text(label).size(10).color(theme::DIM2),
    ]
    .spacing(7)
    .align_y(Alignment::Center);

    if let Some(trailing) = trailing {
        content = content.push(horizontal_space()).push(trailing);
    }

    content.padding(Padding::from([2, 4])).into()
}

/// 侧边栏底部的「设置」入口：与上方导航项使用同一套样式。
fn settings_card(app: &App) -> Element<'_> {
    nav_item(
        "设置",
        "动画与缓存",
        widgets::ICON_SETTINGS,
        app.settings_page,
        Message::OpenSettings,
    )
}

/// 侧边栏底部的「关于」入口：品牌、版本与本地数据说明。
fn about_card(app: &App) -> Element<'_> {
    nav_item(
        "关于",
        "Montis · 山音",
        widgets::ICON_INFO,
        app.about_page,
        Message::OpenAbout,
    )
}

fn user_card(app: &App) -> Element<'_> {
    // 未登录时这张卡就是入口：在内容区打开网易云登录面板。
    let Some(profile) = &app.profile else {
        return mouse_area(
            container(
                row![
                    netease_avatar(app, 34.0),
                    column![
                        text("未登录网易云").size(12).color(theme::FG),
                        text("登录后同步歌单").size(9).color(theme::DIM2),
                    ]
                    .spacing(1),
                    horizontal_space(),
                    text("登录").size(11).color(theme::ACCENT),
                ]
                .spacing(9)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(Padding::from([10, 11]))
            .style(theme::user_tile),
        )
        .on_press(Message::OpenLogin)
        .interaction(mouse::Interaction::Pointer)
        .into();
    };

    let name = profile.display_name().to_string();
    let subtitle = if profile.user_id > 0 {
        format!("UID {}", profile.user_id)
    } else {
        "网易云音乐".to_string()
    };

    container(
        row![
            netease_avatar(app, 34.0),
            column![
                text(name).size(12).color(theme::FG),
                text(subtitle).size(9).color(theme::DIM2),
            ]
            .spacing(1),
            horizontal_space(),
            button(text("退出").size(11).color(theme::DIM))
                .on_press(Message::Logout)
                .padding(Padding::from([6, 10]))
                .style(theme::ghost),
        ]
        .spacing(9)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([10, 11]))
    .style(theme::user_tile)
    .into()
}

/// 网易云头像：已登录用真头像，未登录时先放一个「网」字占位。
fn netease_avatar(app: &App, size: f32) -> Element<'_> {
    if app.profile.is_none() {
        return avatar_placeholder("网".to_string(), size, theme::ACCENT);
    }
    widgets::avatar(app, size)
}

/// 头像还没到手时的占位圆：一个首字 + 渠道色，尺寸与真头像一致。
fn avatar_placeholder(letter: String, size: f32, color: iced::Color) -> Element<'static> {
    container(text(letter).size(size * 0.40).color(iced::Color::WHITE))
        .width(size)
        .height(size)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(Background::Color(color)),
            border: border::rounded(size * 0.5),
            ..container::Style::default()
        })
        .into()
}

fn nav_item(
    title: &'static str,
    subtitle: &'static str,
    icon: &'static [u8],
    active: bool,
    message: Message,
) -> Element<'static> {
    mouse_area(widgets::nav_display(title, subtitle, icon, active, false))
        .on_press(message)
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn main(app: &App) -> Element<'_> {
    let content: Element<'_> = if app.bili_page {
        bili_page(app)
    } else if app.artist.is_some() {
        artist_page(app)
    } else if app.settings_page {
        settings_page(app)
    } else if app.about_page {
        about_page(app)
    } else if app.search_page {
        search_page(app)
    } else if app.daily_page {
        daily_page(app)
    } else if app.fm_page {
        fm_page(app)
    } else {
        match &app.playlist {
            // 歌单页自己处理滚动：标题卡片固定，只有歌曲列表滚动。
            Some(state) => playlist_page(app, state),
            // 没有任何账号时，内容区直接是网易云登录面板（侧边栏仍可用）。
            None if app.netease_login_page => netease_login_page(app),
            // 我的歌单页同理：标题与「全部歌单」行固定，只有歌单网格滚动。
            None => discover_page(app),
        }
    };

    container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(Padding::from([18, 22]))
        .into()
}

/// 网易云登录面板：不再切到独立登录屏，侧边栏始终可用。
fn netease_login_page(app: &App) -> Element<'_> {
    centered_login(crate::views::login::netease_card(app))
}

/// 内容区居中的登录卡：网易云与哔哩哔哩共用。
fn centered_login<'a>(card: Element<'a>) -> Element<'a> {
    container(card)
        .width(Length::Fill)
        .height(Length::Fill)
        .center_x(Length::Fill)
        .center_y(Length::Fill)
        .into()
}

/// 独立的设置页：窄标题条 + 界面动画 / 音频缓存 / 控制台三张卡片。
fn settings_page(app: &App) -> Element<'_> {
    // 顶部留白避开右上角自绘的窗口按钮（与关于页一致）。
    scrollable(
        column![
            widgets::vspace(26.0),
            widgets::title_bar("设置", "外观、动画与本地缓存"),
            animation_card(app),
            cache_card(app),
            dock_card(app),
        ]
        .spacing(16),
    )
    .height(Length::Fill)
    .style(theme::scrollable_hidden)
    .into()
}

/// 卡片标题行：彩色图标块 + 标题 + 状态胶囊 + 一行说明，右侧可挂提示。
fn setting_head<'a>(
    icon: &'static [u8],
    tint: iced::Color,
    title: &'static str,
    badge: Element<'a>,
    hint: &'a str,
    trailing: Option<&'a str>,
) -> Element<'a> {
    let mut head = row![
        icon_tile(icon, tint),
        column![
            row![text(title).size(14).color(theme::FG), badge]
                .spacing(8)
                .align_y(Alignment::Center),
            widgets::hint(hint),
        ]
        .spacing(3),
        horizontal_space(),
    ]
    .spacing(11)
    .align_y(Alignment::Center);

    if let Some(note) = trailing {
        head = head.push(widgets::hint(note));
    }
    head.into()
}

/// 设置卡标题左边的图标块：填色圆角方 + 白色图标。
fn icon_tile(icon: &'static [u8], tint: iced::Color) -> Element<'static> {
    container(widgets::svg_icon(icon, 15.0, iced::Color::WHITE))
        .width(32)
        .height(32)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(Background::Color(tint)),
            border: border::rounded(10.0),
            ..container::Style::default()
        })
        .into()
}

/// 界面动画：速度档位 + 歌词过渡 + 带尾迹的动态预览。
fn animation_card(app: &App) -> Element<'_> {
    let speed = app.ui.animation_speed;

    widgets::card(
        column![
            setting_head(
                widgets::ICON_SETTINGS,
                theme::ACCENT,
                "界面动画",
                widgets::pill(format!("{speed:.1}×"), PillKind::Ok),
                "控制页面切换、加载进度与歌词换句的过渡节奏",
                Some("改动立即生效"),
            ),
            widgets::soft_rule(),
            setting_row("动画速度", "预设档位，点击即可切换", speed_chips(app)),
            lyric_fade_row(app),
            animation_preview(app),
        ]
        .spacing(14),
    )
}

/// 速度档位按钮。
fn speed_chips(app: &App) -> Element<'_> {
    let speed = app.ui.animation_speed;
    let mut options = row![].spacing(8);
    for (value, label) in ANIMATION_PRESETS {
        options = options.push(speed_option(label, value, (speed - value).abs() < 0.01));
    }
    options.into()
}

/// 歌词换句的淡入淡出开关。
fn lyric_fade_row(app: &App) -> Element<'_> {
    setting_row(
        "歌词淡入淡出",
        "换句时渐隐渐显，关闭后直接切换",
        switch(app.ui.lyric_fade, Message::ToggleLyricFade),
    )
}

/// 设置行：左侧标题与说明，右侧控件。三张卡片共用同一种排版。
fn setting_row<'a>(label: &'a str, hint: &'a str, control: Element<'a>) -> Element<'a> {
    row![
        column![text(label).size(12).color(theme::FG), widgets::hint(hint),].spacing(2),
        horizontal_space(),
        control,
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .width(Length::Fill)
    .into()
}

/// 统一的开关按钮。
fn switch<'a>(on: bool, message: Message) -> Element<'a> {
    button(text(if on { "开" } else { "关" }).size(11).color(if on {
        iced::Color::WHITE
    } else {
        theme::FG
    }))
    .on_press(message)
    .padding(Padding::from([3, 14]))
    .style(theme::toggle(on))
    .into()
}

/// 滑杆两端的范围标注。
fn range_labels(left: &str, right: &str) -> Element<'static> {
    row![
        widgets::hint(left.to_string()),
        horizontal_space(),
        widgets::hint(right.to_string()),
    ]
    .width(Length::Fill)
    .into()
}

/// 独立的「关于」页：顶栏里的品牌信息搬到这里，另附版本与本地数据说明。
fn about_page(app: &App) -> Element<'_> {
    let brand = container(
        row![
            container(widgets::logo(64.0))
                .padding(6)
                .style(|_| container::Style {
                    background: Some(Background::Color(iced::Color::WHITE)),
                    border: border::rounded(20.0),
                    ..container::Style::default()
                }),
            column![
                row![
                    text("Montis").size(30).color(iced::Color::WHITE),
                    widgets::brand_pill("v0.1.0"),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
                text("山音 · 网易云 / 哔哩哔哩客户端")
                    .size(12)
                    .color(theme::WHITE_82),
                text("为播放与频谱可视化而生的本地优先客户端。")
                    .size(11)
                    .color(theme::WHITE_70),
            ]
            .spacing(5),
            horizontal_space(),
            widgets::equalizer(
                app.phase,
                20,
                3.0,
                42.0,
                iced::Color::from_rgba(1.0, 1.0, 1.0, 0.85),
            ),
        ]
        .spacing(18)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([26, 28]))
    .style(|_| container::Style {
        background: Some(theme::hero_gradient()),
        border: border::rounded(20.0),
        ..container::Style::default()
    });

    let facts = row![
        fact_card("版本", "v0.1.0", "Rust + iced 0.13"),
        fact_card("数据", "本地优先", "会话与偏好只保存在本机"),
        fact_card("频谱", "72 段", "2048 点 FFT · 环形可视化"),
    ]
    .spacing(14)
    .width(Length::Fill);

    let stack = widgets::card(
        column![
            widgets::section_title("技术栈"),
            info_line("界面", "Rust · iced 0.13，自绘窗口与平铺卡片"),
            info_line("音频", "rodio 解码与输出，rustfft 做实时频谱分析"),
            info_line("接口", "网易云 weapi / 哔哩哔哩 Web 接口与 WBI 签名"),
            info_line(
                "数据",
                "~/.config/melody/ 存会话与偏好，~/.cache/melody/ 存音频与封面缓存，\
                 ~/.local/share/melody/music/ 存本地音乐"
            ),
        ]
        .spacing(10),
    );

    let project = widgets::card(
        column![
            widgets::section_title("项目地址"),
            row![
                container(text("GitHub").size(10).color(theme::DIM2)).width(Length::Fixed(48.0)),
                link(
                    crate::app::PROJECT_URL,
                    theme::ACCENT,
                    Message::OpenProjectUrl,
                ),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
            widgets::hint("点击用系统浏览器打开"),
        ]
        .spacing(8),
    );

    let privacy = widgets::card(
        column![
            row![
                widgets::section_title("本地与隐私"),
                horizontal_space(),
                widgets::pill("不上传偏好", PillKind::Ok),
            ]
            .spacing(8)
            .align_y(Alignment::Center),
            widgets::hint(
                "Cookie 与账号资料只写入本机会话文件（权限 600）；\
                 播放队列、界面动画与控制台透明度也都只保存在本地。",
            ),
            widgets::hint("Montis（山音）是个人项目，与网易云音乐官方无关。"),
        ]
        .spacing(8),
    );

    // 顶部留一条空白：右下角 / 右上角是自绘的窗口按钮，别让品牌卡盖到它们下面。
    scrollable(column![widgets::vspace(26.0), brand, facts, stack, project, privacy].spacing(16))
        .height(Length::Fill)
        .style(theme::scrollable_hidden)
        .into()
}

fn fact_card<'a>(label: &'a str, value: &'a str, note: &'a str) -> Element<'a> {
    container(
        column![
            text(label).size(9).color(theme::DIM2),
            text(value).size(19).color(theme::FG),
            text(note).size(10).color(theme::DIM2),
        ]
        .spacing(4),
    )
    .width(Length::Fill)
    .padding(Padding::from(16))
    .style(theme::card)
    .into()
}

fn info_line<'a>(label: &'a str, value: &'a str) -> Element<'a> {
    row![
        container(text(label).size(10).color(theme::DIM2)).width(Length::Fixed(48.0)),
        text(value).size(12).color(theme::FG),
    ]
    .spacing(12)
    .align_y(Alignment::Center)
    .into()
}

/// 音频缓存：占用条、3 GB ~ 10 GB 的上限滑杆、开关与清空。
fn cache_card(app: &App) -> Element<'_> {
    let limit = app.ui.cache_limit_mb;
    let enabled = limit > 0;

    let toggle = switch(
        enabled,
        Message::SetCacheLimit(if enabled {
            0
        } else {
            crate::audio_cache::DEFAULT_LIMIT_MB
        }),
    );

    let clear = button(text("清空全部缓存").size(11))
        .on_press(Message::ClearCache)
        .padding(Padding::from([6, 14]))
        .style(theme::ghost);

    let mut content = column![
        setting_head(
            widgets::ICON_REFRESH,
            theme::VIOLET,
            "音频缓存",
            widgets::pill(format_bytes(app.cache_usage), PillKind::Muted),
            "听过的歌曲留在本地，封面与首页快照也一并缓存",
            Some("~/.cache/melody"),
        ),
        widgets::soft_rule(),
        usage_bar(app.cache_usage, limit * MB),
        setting_row(
            "本地缓存",
            if enabled {
                "达到上限后自动淘汰最久没听的歌曲"
            } else {
                "已关闭：每次播放都会重新下载"
            },
            toggle,
        ),
    ]
    .spacing(14);

    if enabled {
        content = content.push(cache_slider(app));
    }
    content = content.push(
        row![
            widgets::hint(format!(
                "本地音乐下载独立存放（已用 {}）",
                format_bytes(app.downloads_usage)
            )),
            horizontal_space(),
            clear,
        ]
        .spacing(12)
        .align_y(Alignment::Center),
    );

    widgets::card(content)
}

/// 缓存占用条：填充比例 = 已用 / 上限，两端文字标注具体容量。
fn usage_bar(used: u64, limit: u64) -> Element<'static> {
    let ratio = if limit == 0 {
        0.0
    } else {
        (used as f32 / limit as f32).clamp(0.0, 1.0)
    };
    let filled = (ratio * 1000.0).round() as u16;

    let mut bar: iced::widget::Row<'static, Message> = row![].height(8);
    if filled > 0 {
        bar = bar.push(
            container(row![])
                .width(Length::FillPortion(filled))
                .height(8)
                .style(|_| container::Style {
                    background: Some(theme::linear(
                        90.0,
                        &[(0.0, theme::ACCENT), (1.0, theme::VIOLET)],
                    )),
                    border: border::rounded(4.0),
                    ..container::Style::default()
                }),
        );
    }
    if filled < 1000 {
        bar = bar.push(horizontal_space().width(Length::FillPortion(1000 - filled)));
    }

    let track = container(bar)
        .width(Length::Fill)
        .height(8)
        .style(|_| container::Style {
            background: Some(Background::Color(theme::SUNK)),
            border: border::rounded(4.0),
            ..container::Style::default()
        });

    let note = if limit == 0 {
        "缓存已关闭".to_string()
    } else {
        format!("上限 {}", format_bytes(limit))
    };

    column![
        track,
        row![
            widgets::hint(format!("已用 {}", format_bytes(used))),
            horizontal_space(),
            widgets::hint(note),
        ]
        .width(Length::Fill),
    ]
    .spacing(6)
    .into()
}

/// 3 GB ~ 10 GB 的滑杆，按 1 GB 步进。
fn cache_slider(app: &App) -> Element<'_> {
    let fraction = crate::audio_cache::limit_to_fraction(app.ui.cache_limit_mb);

    column![
        crate::views::player::setting_slider(fraction, |t| {
            Message::SetCacheLimit(crate::audio_cache::fraction_to_limit(t))
        }),
        range_labels("3 GB", "10 GB"),
    ]
    .spacing(2)
    .into()
}

/// 播放页控制台的不透明度：滑杆 + 可直接对比的预览。
fn dock_card(app: &App) -> Element<'_> {
    let opacity = app.ui.dock_opacity;
    let (low, high) = crate::ui_settings::DOCK_OPACITY_RANGE;

    widgets::card(
        column![
            setting_head(
                widgets::ICON_EXPAND,
                theme::NIGHT,
                "播放页控制台",
                widgets::pill(format!("{:.0}%", opacity * 100.0), PillKind::Muted),
                "控制台越低越能透出封面氛围，100% 为实心白",
                Some("实时预览"),
            ),
            widgets::soft_rule(),
            dock_slider(app),
            range_labels(
                &format!("{:.0}%", low * 100.0),
                &format!("{:.0}%", high * 100.0)
            ),
            dock_preview(opacity),
        ]
        .spacing(14),
    )
}

/// 把 0.4~1.0 的不透明度映射到滑杆的 0~1。
fn dock_slider(app: &App) -> Element<'_> {
    let (low, high) = crate::ui_settings::DOCK_OPACITY_RANGE;
    let value = ((app.ui.dock_opacity - low) / (high - low)).clamp(0.0, 1.0);

    crate::views::player::setting_slider(value, move |t| {
        Message::SetDockOpacity(low + t * (high - low))
    })
}

/// 预览：把迷你控制台放在深色氛围上，直接看透明度。
fn dock_preview(opacity: f32) -> Element<'static> {
    let art = container(text("♪").size(14).color(iced::Color::WHITE))
        .width(32)
        .height(32)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|_| container::Style {
            background: Some(theme::brand_gradient()),
            border: border::rounded(9.0),
            ..container::Style::default()
        });

    // 歌名 / 歌手用两条圆角占位条代替，避免在预览里堆真文字。
    let title = container(row![])
        .width(112)
        .height(7)
        .style(|_| container::Style {
            background: Some(Background::Color(iced::Color {
                a: 0.30,
                ..theme::FG
            })),
            border: border::rounded(4.0),
            ..container::Style::default()
        });
    let subtitle = container(row![])
        .width(70)
        .height(5)
        .style(|_| container::Style {
            background: Some(Background::Color(iced::Color {
                a: 0.14,
                ..theme::FG
            })),
            border: border::rounded(3.0),
            ..container::Style::default()
        });

    let controls = row![
        widgets::svg_icon(widgets::ICON_PREV, 10.0, theme::DIM),
        container(widgets::svg_icon(
            widgets::ICON_PLAY,
            9.0,
            iced::Color::WHITE
        ))
        .padding(5)
        .style(|_| container::Style {
            background: Some(theme::button_gradient()),
            border: border::rounded(999.0),
            ..container::Style::default()
        }),
        widgets::svg_icon(widgets::ICON_NEXT, 10.0, theme::DIM),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let mut progress: iced::widget::Row<'static, Message> = row![].height(3);
    progress = progress.push(
        container(row![])
            .width(Length::FillPortion(4))
            .height(3)
            .style(|_| container::Style {
                background: Some(Background::Color(theme::ACCENT)),
                border: border::rounded(2.0),
                ..container::Style::default()
            }),
    );
    progress = progress.push(horizontal_space().width(Length::FillPortion(6)));

    let cloud = container(
        column![
            row![
                art,
                column![title, subtitle].spacing(4),
                horizontal_space(),
                controls,
            ]
            .spacing(10)
            .align_y(Alignment::Center),
            progress,
            row![
                text("0:00").size(9).color(theme::DIM),
                horizontal_space(),
                text("4:29").size(9).color(theme::DIM),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(8)
        .padding(Padding::from([11, 13])),
    )
    .width(Length::Fill)
    .style(move |_| container::Style {
        background: Some(Background::Color(iced::Color {
            a: opacity,
            ..iced::Color::WHITE
        })),
        border: border::rounded(14.0)
            .width(1.0)
            .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.40)),
        text_color: Some(theme::FG),
        ..container::Style::default()
    });

    container(
        row![
            horizontal_space().width(Length::FillPortion(1)),
            cloud,
            horizontal_space().width(Length::FillPortion(1)),
        ]
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from(16))
    .style(|_| container::Style {
        background: Some(Background::Color(theme::NIGHT)),
        border: border::rounded(18.0),
        ..container::Style::default()
    })
    .into()
}

/// 速度档位：选中时用品牌渐变。
fn speed_option<'a>(label: &'a str, value: f32, selected: bool) -> Element<'a> {
    button(text(label).size(12).color(if selected {
        iced::Color::WHITE
    } else {
        theme::FG
    }))
    .on_press(Message::SetAnimationSpeed(value))
    .padding(Padding::from([7, 18]))
    .style(move |_, status| theme::speed_chip(selected, status))
    .into()
}

/// 预览：一颗带头尾光点按当前速度往返，直观感受快慢。
fn animation_preview(app: &App) -> Element<'_> {
    const DOT: f32 = 12.0;
    const GLOW: f32 = 22.0;
    let track = (app.viewport.width - 360.0).clamp(220.0, 760.0);
    let t = (app.clock * 1.6).sin() * 0.5 + 0.5;
    let offset = t * (track - 32.0);

    let dot = |alpha: f32, size: f32| -> Element<'static> {
        container(row![])
            .width(size)
            .height(size)
            .style(move |_| container::Style {
                background: Some(Background::Color(iced::Color {
                    a: alpha,
                    ..theme::ACCENT
                })),
                border: border::rounded(size * 0.5),
                ..container::Style::default()
            })
            .into()
    };

    // 尾迹：三个越来越淡的小点跟在主光点后面。
    // Stack 的尺寸由第一个子元素决定，这里必须先显式定宽，否则后面的
    // 光点会被挤到第一颗尾迹的宽度之外、画不出来。
    let mut layers = iced::widget::stack![]
        .width(Length::Fixed(track - 20.0))
        .height(Length::Fixed(24.0));
    for step in (1..=3).rev() {
        let x = (offset - step as f32 * 10.0).max(0.0);
        layers = layers.push(
            container(row![
                horizontal_space().width(Length::Fixed(x)),
                dot(0.26 - step as f32 * 0.06, DOT * 0.6),
            ])
            .height(Length::Fixed(24.0))
            .align_y(Alignment::Center),
        );
    }
    layers = layers.push(
        container(row![
            horizontal_space().width(Length::Fixed((offset - (GLOW - DOT) * 0.5).max(0.0))),
            widgets::glow(GLOW, theme::ACCENT, 0.55),
        ])
        .height(Length::Fill)
        .align_y(Alignment::Center),
    );
    layers = layers.push(
        container(row![
            horizontal_space().width(Length::Fixed(offset)),
            dot(1.0, DOT),
        ])
        .height(Length::Fill)
        .align_y(Alignment::Center),
    );

    container(layers)
        .width(Length::Fixed(track))
        .height(Length::Fixed(40.0))
        .padding(Padding::from([8, 10]))
        .style(|_| container::Style {
            background: Some(theme::linear(
                90.0,
                &[
                    (0.0, iced::Color::from_rgb(0.937, 0.953, 1.0)),
                    (1.0, iced::Color::from_rgb(0.965, 0.973, 0.988)),
                ],
            )),
            border: border::rounded(20.0),
            ..container::Style::default()
        })
        .into()
}

fn discover_page(app: &App) -> Element<'_> {
    // 标题与「全部歌单」标题行固定，只有下面的歌单网格滚动。
    let mut header = column![
        widgets::vspace(26.0),
        widgets::title_bar("我的歌单", "打开歌单开始浏览，或到左侧搜索想听的歌"),
        library_head(app),
    ]
    .spacing(16);

    if let Some(warning) = &app.storage_error {
        header = header.push(widgets::error_text(warning.clone()));
    }

    column![header, library_grid(app)]
        .spacing(16)
        .height(Length::Fill)
        .into()
}

/// 独立的搜索页：搜索框 + 结果列表，从首页拆出来单独一页。
fn search_page(app: &App) -> Element<'_> {
    // 搜索框与「搜索结果」行固定，只有结果列表滚动（与歌单详情一致）。
    let mut header = column![
        widgets::vspace(26.0),
        widgets::title_bar("搜索", "输入歌名、歌手或专辑，按回车开始搜索"),
        search_bar(app),
    ]
    .spacing(16);

    if app.search.submitted {
        header = header.push(search_results_head(app));
    }

    if let Some(warning) = &app.storage_error {
        header = header.push(widgets::error_text(warning.clone()));
    }

    let body: Element<'_> = if !app.search.submitted {
        widgets::card(widgets::hint(
            "输入关键词后按回车，结果会列在这里；点歌曲即可播放或添加到下一首。",
        ))
    } else if app.search.busy {
        widgets::card(
            row![
                widgets::spinner(app.phase, 3.5, theme::ACCENT),
                widgets::hint("正在搜索…"),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
    } else if let Some(error) = &app.search.error {
        widgets::card(widgets::error_text(error.clone()))
    } else if app.search.results.is_empty() {
        widgets::card(widgets::hint("没有找到相关歌曲，换个关键词试试。"))
    } else {
        let more = ((app.search.offset as u64) < app.search.total).then_some(Message::SearchMore);
        song_list(
            app,
            &app.search.results,
            ScrollArea::Search,
            Message::PlaySearchTrack,
            Message::EnqueueSearchTrack,
            more,
            app.search.more_busy,
        )
    };

    column![header, body]
        .spacing(16)
        .height(Length::Fill)
        .into()
}

/// 每日推荐页：按账号口味每天更新的歌曲列表。
fn daily_page(app: &App) -> Element<'_> {
    // 标题与数量行固定，只有下面的歌曲列表滚动（与歌单详情一致）。
    let mut header = column![
        widgets::vspace(26.0),
        widgets::title_bar("每日推荐", "根据你的口味，每天更新一次"),
        row![
            widgets::section_title("今日推荐"),
            widgets::pill(format!("{} 首", app.daily.songs.len()), PillKind::Muted),
            horizontal_space(),
            widgets::refresh_button(app.daily.busy, app.phase, Message::ReloadDaily),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(14);

    if let Some(warning) = &app.storage_error {
        header = header.push(widgets::error_text(warning.clone()));
    }

    let body: Element<'_> = if app.daily.busy && app.daily.songs.is_empty() {
        widgets::card(
            row![
                widgets::spinner(app.phase, 3.5, theme::ACCENT),
                widgets::hint("正在获取今天的推荐…"),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
    } else if let Some(error) = &app.daily.error {
        widgets::card(
            column![
                widgets::error_text(error.clone()),
                link("重新加载", theme::ACCENT, Message::ReloadDaily),
            ]
            .spacing(8),
        )
    } else if app.daily.songs.is_empty() {
        widgets::card(widgets::hint("今天还没有为你生成推荐歌曲，稍后再来看看。"))
    } else {
        song_list(
            app,
            &app.daily.songs,
            ScrollArea::Daily,
            Message::PlayDailyTrack,
            Message::EnqueueDailyTrack,
            None,
            false,
        )
    };

    column![header, body]
        .spacing(14)
        .height(Length::Fill)
        .into()
}

/// 私人 FM：根据账号口味持续推荐，放到队尾会自动续上新歌。
fn fm_page(app: &App) -> Element<'_> {
    // 与每日推荐一致：标题与数量行固定，只有下面的歌曲列表滚动。
    let mut header = column![
        widgets::vspace(26.0),
        widgets::title_bar("私人FM", "根据你的口味持续推荐，播完会自动续上新歌"),
        row![
            widgets::section_title("为你挑选"),
            widgets::pill(format!("{} 首", app.fm.songs.len()), PillKind::Muted),
            horizontal_space(),
            fm_play_all(app.fm.songs.is_empty()),
            widgets::refresh_button(app.fm.busy, app.phase, Message::ReloadFm),
        ]
        .spacing(8)
        .align_y(Alignment::Center),
    ]
    .spacing(14);

    if let Some(warning) = &app.storage_error {
        header = header.push(widgets::error_text(warning.clone()));
    }

    let body: Element<'_> = if app.fm.busy && app.fm.songs.is_empty() {
        widgets::card(
            row![
                widgets::spinner(app.phase, 3.5, theme::ACCENT),
                widgets::hint("正在为你挑选歌曲…"),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
    } else if let Some(error) = &app.fm.error {
        widgets::card(
            column![
                widgets::error_text(error.clone()),
                link("换一批", theme::ACCENT, Message::ReloadFm),
            ]
            .spacing(8),
        )
    } else if app.fm.songs.is_empty() {
        widgets::card(widgets::hint("暂时没有推荐，点右上角换一批试试。"))
    } else {
        song_list(
            app,
            &app.fm.songs,
            ScrollArea::Fm,
            Message::PlayFmTrack,
            Message::EnqueueFmTrack,
            None,
            false,
        )
    };

    column![header, body]
        .spacing(14)
        .height(Length::Fill)
        .into()
}

/// FM 页的「播放全部」：没有歌曲时置灰，避免点出空队列。
fn fm_play_all(disabled: bool) -> Element<'static> {
    let label = row![
        widgets::svg_icon(widgets::ICON_PLAY, 11.0, iced::Color::WHITE),
        text("播放全部").size(12).color(iced::Color::WHITE),
    ]
    .spacing(6)
    .align_y(Alignment::Center);

    let button = button(label)
        .padding(Padding::from([7, 14]))
        .style(theme::primary);

    if disabled {
        button.into()
    } else {
        button.on_press(Message::PlayAllFm).into()
    }
}

/// 哔哩哔哩分区：未连接时展示 Cookie 表单；连接后展示收藏夹，
/// 点开收藏夹复用歌单页的虚拟化列表与播放逻辑。
fn bili_page(app: &App) -> Element<'_> {
    if let Some(state) = app.playlist.as_ref().filter(|state| state.bilibili) {
        return playlist_page(app, state);
    }

    // 未连接时和网易云登录页一致：内容区只有一张居中的登录卡。
    if !app.bili_session.is_logged_in() {
        return centered_login(crate::views::login::bili_card(app));
    }

    let mut header = column![
        widgets::vspace(26.0),
        widgets::title_bar("哔哩哔哩", "收藏夹里的视频当音频听，不需要打开视频页面",),
    ]
    .spacing(16);

    if let Some(warning) = &app.storage_error {
        header = header.push(widgets::error_text(warning.clone()));
    }

    column![header, bili_folders(app)]
        .spacing(16)
        .height(Length::Fill)
        .into()
}

fn bili_folders(app: &App) -> Element<'_> {
    let all = &app.bili_state.folders;
    let hidden_count = all
        .iter()
        .filter(|folder| app.bili_session.is_folder_hidden(folder.id))
        .count();
    let folders: Vec<&BiliFolder> = all
        .iter()
        .filter(|folder| {
            app.bili_state.show_hidden || !app.bili_session.is_folder_hidden(folder.id)
        })
        .collect();

    // 账号卡片已经在侧边栏展示，这里只放列表标题。
    let mut head = row![
        widgets::section_title("我的收藏夹"),
        widgets::pill(format!("{} 个", folders.len()), PillKind::Muted),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    if hidden_count > 0 {
        head = head.push(link(
            if app.bili_state.show_hidden {
                "收起已隐藏"
            } else {
                "显示已隐藏"
            },
            theme::DIM,
            Message::ToggleShowHiddenBiliFolders,
        ));
    }

    let head = head.push(horizontal_space()).push(widgets::refresh_button(
        app.bili_state.folders_busy,
        app.phase,
        Message::ReloadBiliFolders,
    ));

    let mut body = column![head].spacing(14);

    if let Some(error) = &app.bili_state.folders_error {
        body = body.push(widgets::card(
            column![
                widgets::error_text(error.clone()),
                link("重新加载", theme::ACCENT, Message::ReloadBiliFolders),
            ]
            .spacing(8),
        ));
    }

    let grid: Element<'_> = if app.bili_state.folders_busy && all.is_empty() {
        widgets::card(
            row![
                widgets::spinner(app.phase, 3.5, theme::ACCENT),
                widgets::hint("正在读取收藏夹…"),
            ]
            .spacing(12)
            .align_y(Alignment::Center),
        )
    } else if folders.is_empty() {
        if hidden_count > 0 {
            widgets::card(widgets::hint(
                "收藏夹都被隐藏了。点上方「显示已隐藏」可以看到并恢复。",
            ))
        } else {
            widgets::card(widgets::hint(
                "还没有收藏夹。先在 bilibili.com 收藏几个视频，再回来刷新。",
            ))
        }
    } else {
        let available = (app.viewport.width - 250.0 - 44.0 - 12.0).max(220.0);
        let (columns, card_width) = grid_metrics(available);
        let mut grid = column![].spacing(14.0);
        for chunk in folders.chunks(columns) {
            let mut cards = row![].spacing(14.0);
            for folder in chunk {
                cards = cards.push(bili_folder_card(app, folder, card_width));
            }
            grid = grid.push(cards);
        }
        scrollable(container(grid).width(Length::Fill).padding(Padding {
            top: 2.0,
            right: 12.0,
            bottom: 16.0,
            left: 0.0,
        }))
        .height(Length::Fill)
        .on_scroll(|_| Message::Scrolled(ScrollArea::Library))
        .style(theme::scrollable_auto(
            app.scroll_opacity(ScrollArea::Library),
        ))
        .into()
    };

    body = body.push(grid);
    body.height(Length::Fill).into()
}

fn bili_user_card(app: &App) -> Element<'_> {
    // 未连接时这张卡就是入口：点开哔哩哔哩页粘贴 Cookie。
    if !app.bili_session.is_logged_in() {
        return mouse_area(
            container(
                row![
                    bili_avatar(app, 34.0),
                    column![
                        text("未连接哔哩哔哩").size(12).color(theme::FG),
                        text("登录后听收藏夹").size(9).color(theme::DIM2),
                    ]
                    .spacing(1),
                    horizontal_space(),
                    text("连接").size(11).color(theme::ACCENT),
                ]
                .spacing(9)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(Padding::from([10, 11]))
            .style(theme::user_tile),
        )
        .on_press(Message::OpenBilibili)
        .interaction(mouse::Interaction::Pointer)
        .into();
    }

    let name = if app.bili_session.name.is_empty() {
        "哔哩哔哩用户"
    } else {
        app.bili_session.name.as_str()
    };
    let subtitle = if app.bili_session.mid > 0 {
        format!("UID {}", app.bili_session.mid)
    } else {
        "哔哩哔哩".to_string()
    };

    container(
        row![
            bili_avatar(app, 34.0),
            column![
                text(name).size(12).color(theme::FG),
                text(subtitle).size(9).color(theme::DIM2),
            ]
            .spacing(1),
            horizontal_space(),
            button(text("退出").size(11).color(theme::DIM))
                .on_press(Message::BiliLogout)
                .padding(Padding::from([6, 10]))
                .style(theme::ghost),
        ]
        .spacing(9)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([10, 11]))
    .style(theme::user_tile)
    .into()
}

/// 哔哩哔哩头像：复用封面缓存，600 尺寸会额外生成圆形版本；
/// 图还没下载完时先显示昵称首字，尺寸不会跳。
fn bili_avatar(app: &App, size: f32) -> Element<'_> {
    let url = nm_api::sized_cover(&app.bili_session.face, 600);
    let handle = app.circle_covers.get(&url).or_else(|| app.covers.get(&url));
    if let Some(handle) = handle {
        return image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Fill)
            .into();
    }

    let initial = app
        .bili_session
        .name
        .chars()
        .next()
        .map(|letter| letter.to_string())
        .unwrap_or_else(|| "B".to_string());
    avatar_placeholder(initial, size, iced::Color::from_rgb(0.976, 0.376, 0.545))
}

fn bili_folder_card<'a>(app: &'a App, folder: &'a BiliFolder, width: f32) -> Element<'a> {
    let cover_size = (width - 24.0).max(120.0);
    let hidden = app.bili_session.is_folder_hidden(folder.id);
    let art = cover_with_badge(app, &folder.cover(300), cover_size, &folder.title);

    let mut tags = row![
        widgets::pill(format!("{} 个视频", folder.media_count), PillKind::Muted),
        if folder.attr == 0 {
            widgets::pill("公开", PillKind::Ok)
        } else {
            widgets::pill("私密", PillKind::Warn)
        },
    ]
    .spacing(6);
    if hidden {
        tags = tags.push(widgets::pill("已隐藏", PillKind::Warn));
    }

    let body = column![
        art,
        text(clip(&folder.title, 16)).size(13).color(theme::FG),
        tags,
    ]
    .spacing(9);

    let card = button(body)
        .on_press(Message::OpenBiliFolder(folder.id))
        .width(Length::Fixed(width))
        .padding(12)
        .style(theme::playlist_card);

    // 封面右上角的隐藏 / 恢复入口。放在 `stack` 顶层，点击它不会
    // 穿透到下面的卡片按钮，其它位置仍然可以正常打开收藏夹。
    let toggle = mouse_area(
        container(
            text(if hidden { "显示" } else { "隐藏" })
                .size(9)
                .color(iced::Color::WHITE),
        )
        .padding(Padding::from([3, 8]))
        .style(|_| container::Style {
            background: Some(Background::Color(iced::Color::from_rgba(
                0.07, 0.09, 0.16, 0.62,
            ))),
            border: border::rounded(999.0),
            ..container::Style::default()
        }),
    )
    .on_press(Message::ToggleBiliFolderHidden(folder.id))
    .interaction(mouse::Interaction::Pointer);

    let overlay = container(toggle)
        .width(Length::Fixed(width))
        .height(Length::Fixed(cover_size + 24.0))
        .align_x(Alignment::End)
        .align_y(Alignment::Start)
        .padding(Padding {
            top: 20.0,
            right: 20.0,
            bottom: 0.0,
            left: 0.0,
        });

    iced::widget::stack![card, overlay].into()
}

/// 歌手页：头像 + 简介固定在上方，歌曲列表单独滚动。
fn artist_page(app: &App) -> Element<'_> {
    let Some(state) = &app.artist else {
        return widgets::card(widgets::hint("没有可展示的歌手。"));
    };

    let back = mouse_area(
        row![
            text("←").size(14).color(theme::ACCENT),
            text("返回").size(12).color(theme::ACCENT),
        ]
        .spacing(6)
        .align_y(Alignment::Center),
    )
    .on_press(Message::CloseArtist)
    .interaction(mouse::Interaction::Pointer);

    if state.busy && state.detail.is_none() {
        return column![
            back,
            widgets::card(
                row![
                    widgets::spinner(app.phase, 4.0, theme::ACCENT),
                    widgets::hint("正在加载歌手资料…"),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            ),
        ]
        .spacing(14)
        .into();
    }

    if let Some(error) = &state.error {
        return column![
            back,
            widgets::card(
                column![
                    widgets::error_text(error.clone()),
                    link("重试", theme::ACCENT, Message::OpenArtist(state.id)),
                ]
                .spacing(8),
            ),
        ]
        .spacing(14)
        .into();
    }

    let Some(detail) = &state.detail else {
        return column![
            back,
            widgets::card(widgets::hint("这个歌手暂时没有公开资料。")),
        ]
        .spacing(14)
        .into();
    };

    let header = column![back, artist_header(app, detail, state.bio_expanded),].spacing(14);

    let count = if state.total > state.songs.len() as u64 {
        format!("{} / {} 首", state.songs.len(), state.total)
    } else {
        format!("{} 首", state.songs.len())
    };
    let songs_head = row![
        widgets::section_title("热门歌曲"),
        widgets::pill(count, PillKind::Muted),
        horizontal_space(),
        text("按热度排序").size(10).color(theme::DIM2),
    ]
    .spacing(8)
    .align_y(Alignment::Center);

    let body: Element<'_> = if state.songs.is_empty() {
        widgets::card(widgets::hint("暂时没有可播放的歌曲。"))
    } else {
        let more = ((state.offset as u64) < state.total).then_some(Message::ArtistMore);
        song_list(
            app,
            &state.songs,
            ScrollArea::Artist,
            Message::PlayArtistTrack,
            Message::EnqueueArtistTrack,
            more,
            state.more_busy,
        )
    };

    column![header, songs_head, body]
        .spacing(14)
        .height(Length::Fill)
        .into()
}

/// 歌手资料卡：圆形头像 + 名字 / 别名 / 作品数 + 简介。
fn artist_header<'a>(app: &'a App, detail: &'a ArtistDetail, expanded: bool) -> Element<'a> {
    let size = 104.0;
    let url = nm_api::sized_cover(detail.image(), 600);
    let art: Element<'a> = if let Some(handle) = app.circle_covers.get(&url) {
        image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Fill)
            .into()
    } else if let Some(handle) = app.covers.get(&url) {
        image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Fill)
            .into()
    } else {
        artist_placeholder(&detail.name, size)
    };

    let alias = detail.subtitle();
    let eyebrow = row![
        widgets::dot(theme::ACCENT, 5.0),
        text("ARTIST").size(9).color(theme::DIM2),
    ]
    .spacing(7)
    .align_y(Alignment::Center);

    let alias_row: Element<'a> = if alias.is_empty() {
        row![].into()
    } else {
        text(alias).size(11).color(theme::DIM).into()
    };

    let stats = row![
        widgets::pill(format!("{} 首歌曲", detail.music_size), PillKind::Muted),
        widgets::pill(format!("{} 张专辑", detail.album_size), PillKind::Muted),
        widgets::pill(format!("{} 个 MV", detail.mv_size), PillKind::Muted),
    ]
    .spacing(6);

    let bio = detail.brief_desc.trim();
    let can_toggle = bio.chars().count() > 180;
    let shown = if expanded || !can_toggle {
        bio.to_string()
    } else {
        clip(bio, 180)
    };
    let mut bio_column =
        column![text(shown).size(11).color(theme::DIM2).width(Length::Fill)].spacing(4);
    if can_toggle {
        bio_column = bio_column.push(link(
            if expanded {
                "收起简介"
            } else {
                "展开简介"
            },
            theme::ACCENT,
            Message::ToggleArtistBio,
        ));
    }

    let actions: Element<'a> = if detail.music_size == 0 {
        row![].into()
    } else {
        button(
            row![
                widgets::svg_icon(widgets::ICON_PLAY, 11.0, iced::Color::WHITE),
                text("播放全部").size(12).color(iced::Color::WHITE),
            ]
            .spacing(6)
            .align_y(Alignment::Center),
        )
        .on_press(Message::PlayArtistTrack(0))
        .padding(Padding::from([9, 18]))
        .style(theme::primary)
        .into()
    };

    let meta = column![
        eyebrow,
        text(detail.name.clone()).size(26).color(theme::FG),
        alias_row,
        stats,
        bio_column,
        actions,
    ]
    .spacing(8)
    .width(Length::Fill);

    container(row![art, meta].spacing(20).align_y(Alignment::Center))
        .width(Length::Fill)
        .padding(Padding::from([18, 20]))
        .style(theme::feature_card)
        .into()
}

/// 头像还没下载好时的占位：品牌色圆形 + 名字首字。
fn artist_placeholder<'a>(name: &str, size: f32) -> Element<'a> {
    let initial = name
        .chars()
        .next()
        .map(|character| character.to_uppercase().to_string())
        .unwrap_or_else(|| "♪".to_string());

    container(text(initial).size(size * 0.34).color(theme::WHITE_82))
        .width(size)
        .height(size)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(move |_| container::Style {
            background: Some(theme::brand_gradient()),
            border: border::rounded(size / 2.0),
            ..container::Style::default()
        })
        .into()
}

/// 「搜索结果」标题行：数量与清空按钮始终可见，不随结果列表滚走。
fn search_results_head(app: &App) -> Element<'_> {
    let loaded = app.search.results.len() as u64;
    let count = if app.search.total > loaded {
        format!("{loaded} / {} 首", app.search.total)
    } else {
        format!("{loaded} 首")
    };

    row![
        widgets::section_title("搜索结果"),
        widgets::pill(count, PillKind::Muted),
        horizontal_space(),
        link("清空", theme::DIM, Message::ClearSearch),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// 滚动到距底部多少像素内就自动翻页。
const AUTO_LOAD_MARGIN: f32 = 160.0;

/// 滚动位置是否已经接近底部。
fn near_bottom(viewport: scrollable::Viewport) -> bool {
    let offset = viewport.absolute_offset().y;
    let visible = viewport.bounds().height;
    let content = viewport.content_bounds().height;
    offset + visible + AUTO_LOAD_MARGIN >= content
}

/// 歌曲列表卡片：表头固定，行高一致，整块滚动（每日推荐 / 私人FM / 搜索结果共用）。
fn song_list<'a>(
    app: &'a App,
    songs: &'a [Song],
    area: ScrollArea,
    on_press: fn(usize) -> Message,
    on_enqueue: fn(usize) -> Message,
    more: Option<Message>,
    more_busy: bool,
) -> Element<'a> {
    let mut rows = column![].spacing(0);
    for (index, song) in songs.iter().enumerate() {
        // 每行固定高度，分隔线算在行内，列表滚动时不会抖。
        rows = rows.push(
            container(column![
                song_row(app, index, song, on_press(index), on_enqueue(index), false),
                widgets::soft_rule(),
            ])
            .width(Length::Fill)
            .height(Length::Fixed(TRACK_ROW_HEIGHT)),
        );
    }

    // 没有按钮：滚到底部自动加载下一页，加载中只在底部显示一条提示。
    if more_busy {
        rows = rows.push(loading_row(app.phase));
    }

    let list = scrollable(container(rows).padding(Padding {
        top: 4.0,
        right: 18.0,
        bottom: 16.0,
        left: 18.0,
    }))
    .height(Length::Fill)
    .on_scroll(move |viewport| match &more {
        Some(message) if near_bottom(viewport) => message.clone(),
        _ => Message::Scrolled(area),
    })
    .style(theme::scrollable_auto(app.scroll_opacity(area)));

    container(column![
        list_header(track_header(false)),
        widgets::soft_rule(),
        list
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::card)
    .into()
}

/// 自动翻页时贴在列表底部的加载提示（没有按钮）。
fn loading_row(phase: f32) -> Element<'static> {
    container(
        row![
            widgets::spinner(phase, 3.0, theme::ACCENT),
            widgets::hint("正在加载更多…"),
        ]
        .spacing(10)
        .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([14, 0]))
    .align_x(Alignment::Center)
    .into()
}

fn search_bar(app: &App) -> Element<'_> {
    let input = text_input("搜索歌曲、歌手或专辑", app.search.input.as_str())
        .on_input(Message::SearchChanged)
        .on_submit(Message::SearchSubmit)
        .padding(11)
        .size(13)
        .width(Length::Fill)
        .style(theme::input);

    let submit = button(text("搜索").size(13).color(iced::Color::WHITE))
        .on_press(Message::SearchSubmit)
        .padding(Padding::from([10, 22]))
        .style(theme::primary);

    let mark = container(text("♪").size(14).color(iced::Color::WHITE))
        .width(30)
        .height(30)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|_| container::Style {
            background: Some(theme::button_gradient()),
            border: border::rounded(9.0),
            ..container::Style::default()
        });

    let field = row![mark, input, submit]
        .spacing(10)
        .align_y(Alignment::Center);

    widgets::card(field)
}

/// 「全部歌单」标题行：数量与刷新按钮始终在可视区，不随网格滚走。
fn library_head(app: &App) -> Element<'_> {
    row![
        widgets::section_title("全部歌单"),
        widgets::pill(
            format!("{} 个", app.library.playlists.len()),
            PillKind::Muted
        ),
        horizontal_space(),
        widgets::refresh_button(app.library.busy, app.phase, Message::ReloadLibrary),
    ]
    .spacing(8)
    .align_y(Alignment::Center)
    .into()
}

/// 歌单网格：整块滚动，右侧留出滚动条余量，卡片不会被它压住。
fn library_grid(app: &App) -> Element<'_> {
    let items = &app.library.playlists;

    // 按窗口宽度计算列数，卡片均分整行，宽屏（含全屏）不再留出大片空白。
    // 侧边栏 250 + 主区左右 padding 44 + 滚动条余量。
    let available = (app.viewport.width - 250.0 - 44.0 - 12.0).max(220.0);
    let (columns, card_width) = grid_metrics(available);

    let mut grid = column![].spacing(14.0);
    for chunk in items.chunks(columns) {
        let mut cards = row![].spacing(14.0);
        for item in chunk {
            cards = cards.push(playlist_card(app, item, card_width));
        }
        grid = grid.push(cards);
    }

    // 云端歌单加载失败时仍保留错误提示，下面继续展示已有卡片。
    let mut body = column![].spacing(14);
    if let Some(error) = &app.library.error {
        body = body.push(widgets::card(
            column![
                widgets::error_text(error.clone()),
                link("重新加载", theme::ACCENT, Message::ReloadLibrary),
            ]
            .spacing(8),
        ));
    }
    if items.is_empty() {
        if app.profile.is_none() {
            body = body.push(widgets::card(
                column![
                    widgets::hint("还没有登录网易云，歌单需要登录后才能加载。"),
                    widgets::hint(
                        "可以从左侧「哔哩哔哩」听收藏夹，或登录网易云后使用歌单、搜索、每日推荐与私人 FM。"
                    ),
                    row![widgets::primary_button(
                        "登录网易云",
                        None,
                        Message::OpenLogin
                    )],
                ]
                .spacing(10),
            ));
        } else {
            body = body.push(widgets::card(widgets::hint(
                "还没有歌单。登录后创建或收藏的歌单会出现在这里。",
            )));
        }
    } else {
        body = body.push(grid);
    }

    scrollable(container(body).width(Length::Fill).padding(Padding {
        top: 2.0,
        right: 12.0,
        bottom: 16.0,
        left: 0.0,
    }))
    .height(Length::Fill)
    .on_scroll(|_| Message::Scrolled(ScrollArea::Library))
    .style(theme::scrollable_auto(
        app.scroll_opacity(ScrollArea::Library),
    ))
    .into()
}

/// 根据可用宽度算列数，并把剩余宽度均分给卡片。
///
/// 列数只看宽度，不随卡片数量缩小：只有一两张时也保持正常大小，
/// 不会被拉成整行宽的大图。
fn grid_metrics(available: f32) -> (usize, f32) {
    const SPACING: f32 = 14.0;
    const MIN_CARD: f32 = 210.0;

    let columns = (((available + SPACING) / (MIN_CARD + SPACING)).floor() as usize).max(1);
    let card_width = ((available - SPACING * (columns - 1) as f32) / columns as f32).max(MIN_CARD);

    (columns, card_width)
}

fn playlist_card<'a>(app: &'a App, item: &PlaylistSummary, width: f32) -> Element<'a> {
    // 卡片内边距是 12，封面等宽铺满。
    let cover_size = (width - 24.0).max(120.0);
    let art = cover_with_badge(app, &item.cover(300), cover_size, &item.name);

    let body = column![
        art,
        text(clip(&item.name, 16)).size(13).color(theme::FG),
        row![
            widgets::pill(format!("{} 首", item.track_count), PillKind::Muted),
            if item.special_type == 5 {
                widgets::pill("喜欢", PillKind::Like)
            } else if item.subscribed {
                widgets::pill("收藏", PillKind::Ok)
            } else {
                widgets::pill("创建", PillKind::Muted)
            },
        ]
        .spacing(6),
    ]
    .spacing(9);

    button(body)
        .on_press(Message::OpenPlaylist(item.id))
        .width(Length::Fixed(width))
        .padding(12)
        .style(theme::playlist_card)
        .into()
}

/// 封面右下角的玻璃播放标记，让卡片一眼看出可以点开播放。
fn cover_with_badge<'a>(app: &App, url: &str, size: f32, label: &str) -> Element<'a> {
    let badge = container(text("▶").size(9).color(iced::Color::WHITE))
        .width(26)
        .height(26)
        .align_x(Alignment::Center)
        .align_y(Alignment::Center)
        .style(|_| container::Style {
            background: Some(Background::Color(iced::Color::from_rgb(
                0.102, 0.125, 0.220,
            ))),
            border: border::rounded(999.0)
                .width(1.0)
                .color(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.40)),
            ..container::Style::default()
        });

    let overlay = container(row![horizontal_space(), badge])
        .width(size)
        .height(size)
        .align_y(Alignment::End)
        .padding(8);

    iced::widget::stack![cover(app, url, size, label), overlay].into()
}

fn playlist_page<'a>(app: &'a App, state: &'a PlaylistState) -> Element<'a> {
    let loading_text = if state.bilibili {
        "正在读取收藏夹…"
    } else {
        "正在加载歌单详情…"
    };

    if state.busy {
        let content = column![
            widgets::vspace(14.0),
            widgets::card(
                row![
                    widgets::spinner(app.phase, 4.0, theme::ACCENT),
                    widgets::hint(loading_text),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            ),
        ]
        .spacing(14);
        return content.into();
    }

    if let Some(error) = &state.error {
        let retry = if state.bilibili {
            Message::OpenBiliFolder(state.id)
        } else {
            Message::OpenPlaylist(state.id)
        };
        let content = column![
            widgets::vspace(14.0),
            widgets::card(
                column![
                    widgets::error_text(error.clone()),
                    link("重试", theme::ACCENT, retry),
                ]
                .spacing(8),
            ),
        ]
        .spacing(14);
        return content.into();
    }

    let Some(detail) = &state.detail else {
        let content = column![
            widgets::vspace(14.0),
            widgets::card(widgets::hint("这个歌单暂时没有内容。")),
        ]
        .spacing(14);
        return content.into();
    };

    // 标题卡片固定在顶部，不随歌曲列表滚动。
    // 顶部留白与本地页面一致，避免标题卡顶到窗口按钮下面。
    let header = column![widgets::vspace(14.0), playlist_header(app, state, detail),].spacing(14);
    let list: Element<'_> = if state.local_liked && app.liked.is_empty() {
        widgets::card(
            column![
                widgets::hint("还没有收藏歌曲。"),
                widgets::hint("点击任意歌曲行右侧的红心，收藏会保存在本地，不会同步到网易云。"),
                widgets::hint("也可以点上方「导入」，从之前导出的备份里恢复收藏。"),
            ]
            .spacing(8),
        )
    } else if state.local_downloads && app.local_song_count() == 0 {
        if app.local_scan_busy {
            widgets::card(
                row![
                    widgets::spinner(app.phase, 3.5, theme::ACCENT),
                    widgets::hint("正在扫描音乐文件夹…"),
                ]
                .spacing(12)
                .align_y(Alignment::Center),
            )
        } else {
            widgets::card(
                column![
                    widgets::hint("还没有本地歌曲。"),
                    widgets::hint("点上方「文件夹」添加一个存放音频的目录，会递归扫描子目录。"),
                    widgets::hint("也可以在任意歌曲行点击下载按钮（↓），保存到本地音乐。"),
                    widgets::hint(
                        "支持 mp3 / flac / wav / ogg / m4a / aac，并会读取标签与内嵌封面。"
                    ),
                ]
                .spacing(8),
            )
        }
    } else {
        track_list(app, state, detail)
    };

    column![header, list]
        .spacing(14)
        .width(Length::Fill)
        .height(Length::Fill)
        .into()
}

fn playlist_header<'a>(
    app: &'a App,
    state: &'a PlaylistState,
    detail: &'a PlaylistDetail,
) -> Element<'a> {
    let local = state.local_liked;
    let downloaded = state.local_downloads;
    let bilibili = state.bilibili;
    let cover_size = 158.0;
    // 本地收藏的标题卡用账号头像；普通歌单 / 本地音乐用歌曲封面。
    let art: Element<'a> = if local {
        widgets::avatar(app, cover_size)
    } else {
        cover(app, &detail.cover(420), cover_size, &detail.name)
    };

    let (dot_color, eyebrow_label) = if local {
        (theme::LIKE, "LOCAL")
    } else if downloaded {
        (theme::ACCENT, "LOCAL")
    } else if bilibili {
        (theme::ACCENT, "BILIBILI")
    } else {
        (theme::ACCENT, "PLAYLIST")
    };
    let mut eyebrow = row![
        widgets::dot(dot_color, 5.0),
        text(eyebrow_label).size(9).color(theme::DIM2),
        widgets::pill(format!("{} 首", detail.track_count), PillKind::Muted),
    ]
    .spacing(7)
    .align_y(Alignment::Center);
    if downloaded && app.local_scan_busy {
        eyebrow = eyebrow.push(widgets::pill("扫描中…", PillKind::Warn));
    }

    let mut actions = row![].spacing(10).align_y(Alignment::Center);
    if !detail.tracks.is_empty() {
        actions = actions.push(
            button(
                row![
                    widgets::svg_icon(widgets::ICON_PLAY, 11.0, iced::Color::WHITE),
                    text("播放全部").size(12).color(iced::Color::WHITE),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .on_press(Message::PlayPlaylistTrack(0))
            .padding(Padding::from([9, 18]))
            .style(theme::primary),
        );
        actions = actions.push(
            button(
                row![
                    widgets::svg_icon(
                        widgets::ICON_SHUFFLE,
                        11.0,
                        if app.player.mode.shuffle {
                            iced::Color::WHITE
                        } else {
                            theme::ACCENT_DARK
                        },
                    ),
                    text("随机播放").size(12).color(if app.player.mode.shuffle {
                        iced::Color::WHITE
                    } else {
                        theme::ACCENT_DARK
                    }),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .on_press(Message::ShufflePlaylist)
            .padding(Padding::from([9, 16]))
            .style(theme::soft_accent_button(app.player.mode.shuffle)),
        );
    }

    // 本地收藏可以备份 / 迁移：导出到 JSON，或从 JSON 合并回来。
    if local {
        actions = actions.push(
            button(text("导入").size(12).color(theme::ACCENT_DARK))
                .on_press(Message::OpenLikedTransfer(LikedTransferKind::Import))
                .padding(Padding::from([9, 14]))
                .style(theme::soft_accent_button(false)),
        );
        actions = actions.push(
            button(text("导出").size(12).color(theme::ACCENT_DARK))
                .on_press(Message::OpenLikedTransfer(LikedTransferKind::Export))
                .padding(Padding::from([9, 14]))
                .style(theme::soft_accent_button(false)),
        );
    }

    // 本地音乐：添加 / 管理音乐文件夹。
    if downloaded {
        actions = actions.push(
            button(
                row![
                    widgets::svg_icon(widgets::ICON_FOLDER, 12.0, theme::ACCENT_DARK),
                    text("文件夹").size(12).color(theme::ACCENT_DARK),
                ]
                .spacing(6)
                .align_y(Alignment::Center),
            )
            .on_press(Message::OpenLocalFolders)
            .padding(Padding::from([9, 14]))
            .style(theme::soft_accent_button(false)),
        );
    }

    // 本地音乐：一次性清空磁盘上的下载文件（弹窗里再确认一次）。
    // 音乐文件夹里的文件不属于下载，不会被删掉。
    if downloaded && !app.download_tracks.is_empty() {
        actions = actions.push(
            button(text("清空下载").size(12).color(theme::ERR))
                .on_press(Message::AskClearDownloads)
                .padding(Padding::from([9, 14]))
                .style(theme::ghost),
        );
    }

    let meta: Element<'a> = if local {
        row![
            text("保存在本地 · 不会同步到网易云")
                .size(12)
                .color(theme::DIM),
            text("·").size(11).color(theme::LINE),
            text(format!("{} 首", detail.tracks.len()))
                .size(12)
                .color(theme::DIM),
        ]
        .spacing(7)
        .align_y(Alignment::Center)
        .into()
    } else if downloaded {
        row![
            text("下载 + 音乐文件夹 · 断网也能播放")
                .size(12)
                .color(theme::DIM),
            text("·").size(11).color(theme::LINE),
            text(format!("{} 首", detail.tracks.len()))
                .size(12)
                .color(theme::DIM),
            text("·").size(11).color(theme::LINE),
            text(format_bytes(app.local_total_usage()))
                .size(12)
                .color(theme::DIM),
        ]
        .spacing(7)
        .align_y(Alignment::Center)
        .into()
    } else if bilibili {
        row![
            text("哔哩哔哩收藏夹").size(12).color(theme::DIM),
            text("·").size(11).color(theme::LINE),
            text(format!("{} 首", detail.track_count))
                .size(12)
                .color(theme::DIM),
        ]
        .spacing(7)
        .align_y(Alignment::Center)
        .into()
    } else {
        row![
            text(format!("by {}", detail.creator_name()))
                .size(12)
                .color(theme::DIM),
            text("·").size(11).color(theme::LINE),
            text(format!("{} 次播放", compact(detail.play_count)))
                .size(12)
                .color(theme::DIM),
        ]
        .spacing(7)
        .align_y(Alignment::Center)
        .into()
    };

    // 本地音乐把保存位置与文件夹数量写进描述行，方便用户找到文件。
    let description = if downloaded {
        if app.local_folders.is_empty() {
            format!(
                "下载保存在 {} · 点「文件夹」添加本地音乐目录",
                compact_home_path(&app.paths.music_dir())
            )
        } else {
            format!(
                "已添加 {} 个文件夹 · 下载保存在 {}",
                app.local_folders.len(),
                compact_home_path(&app.paths.music_dir())
            )
        }
    } else {
        clip(detail.description_or_default(), 120)
    };

    let meta_column = column![
        eyebrow,
        text(detail.name.clone()).size(26).color(theme::FG),
        meta,
        text(description)
            .size(11)
            .color(theme::DIM2)
            .width(Length::Fill),
        actions,
    ]
    .spacing(9)
    .width(Length::Fill);

    container(
        row![art, meta_column]
            .spacing(20)
            .align_y(Alignment::Center),
    )
    .width(Length::Fill)
    .padding(Padding::from([18, 20]))
    .style(theme::feature_card)
    .into()
}

/// 列表卡片顶部的表头：跟着卡片一起收圆角，否则会盖掉上面两个圆角。
fn list_header<'a>(content: Element<'a>) -> Element<'a> {
    container(content)
        .width(Length::Fill)
        .padding(Padding::from([10, 18]))
        .style(|_| container::Style {
            background: Some(Background::Color(iced::Color::from_rgb(
                0.961, 0.969, 0.996,
            ))),
            border: border::rounded(border::top(13.0)),
            ..container::Style::default()
        })
        .into()
}

/// 歌单歌曲行的固定高度：内容 50px + 1px 分隔线（虚拟化按它计算占位）。
pub(crate) const TRACK_ROW_HEIGHT: f32 = 51.0;
/// 虚拟化时在视口上下额外保留的行数，减少快速滚动时的空白。
pub(crate) const TRACK_OVERSCAN: usize = 6;
/// 歌单列表的滚动容器 ID：搜索关键词变化时用它把列表滚回顶部。
pub(crate) const PLAYLIST_SCROLL_ID: &str = "playlist-tracks";

/// 歌单内搜索：歌名 / 歌手 / 专辑任意一项包含关键词（关键词已小写化）。
fn song_matches(song: &Song, needle: &str) -> bool {
    song.name.to_lowercase().contains(needle)
        || song.artist_names().to_lowercase().contains(needle)
        || song.album_name().to_lowercase().contains(needle)
}

/// 当前搜索条件下命中的原始下标；关键词为空时返回 `None`（表示全部）。
pub(crate) fn filtered_track_indices(
    state: &PlaylistState,
    detail: &PlaylistDetail,
) -> Option<Vec<usize>> {
    let keyword = state.search.trim().to_lowercase();
    if keyword.is_empty() {
        return None;
    }

    Some(
        detail
            .tracks
            .iter()
            .enumerate()
            .filter(|(_, song)| song_matches(song, &keyword))
            .map(|(index, _)| index)
            .collect(),
    )
}

/// 歌单歌曲列表：只渲染可见行，几千首也不卡。
fn track_list<'a>(
    app: &'a App,
    state: &'a PlaylistState,
    detail: &'a PlaylistDetail,
) -> Element<'a> {
    // 歌单内搜索：关键词为空时展示全部；有词时只保留歌名 / 歌手 /
    // 专辑命中的行，但仍用原始下标去播放与入队。
    let keyword = state.search.trim().to_lowercase();
    let filtered = filtered_track_indices(state, detail);
    let total = filtered.as_ref().map_or(detail.tracks.len(), Vec::len);

    let view_height = app.viewport.height.max(TRACK_ROW_HEIGHT);
    let (first, last, top, bottom) = scroll_window::visible_window(
        total,
        app.playlist_scroll,
        view_height,
        TRACK_ROW_HEIGHT,
        TRACK_OVERSCAN,
    );

    // 行高固定，用占位空间补足屏幕外的行：总高度恒等于「总行数 × 行高」，
    // 所以再长的歌单（几千首）也只渲染看得见的那几十行。
    let mut rows = column![].spacing(0);
    if top > 0.0 {
        rows = rows.push(vertical_space().height(top));
    }
    for position in first..last {
        let index = filtered
            .as_ref()
            .map_or(position, |indices| indices[position]);
        let song = &detail.tracks[index];
        rows = rows.push(
            container(column![
                song_row(
                    app,
                    index,
                    song,
                    Message::PlayPlaylistTrack(index),
                    Message::EnqueuePlaylistTrack(index),
                    state.local_downloads,
                ),
                widgets::soft_rule(),
            ])
            .width(Length::Fill)
            .height(Length::Fixed(TRACK_ROW_HEIGHT)),
        );
    }
    if bottom > 0.0 {
        rows = rows.push(vertical_space().height(bottom));
    }
    // 收藏夹加载新一页时，只在底部显示一条提示；翻页由滚到底部自动触发。
    if state.bilibili && state.bili_busy {
        rows = rows.push(loading_row(app.phase));
    }

    // 列标题固定在滚动区外，列表滚动时依然能对照专辑与时长。
    let header = list_header(track_header(state.local_downloads));

    let bili_more = state.bilibili && state.bili_has_more && !state.bili_busy;
    let list: Element<'a> = if total == 0 {
        let loading = state.filling || state.bili_busy;
        let message = if !keyword.is_empty() {
            if loading {
                "正在加载剩余歌曲，搜索结果稍后更新…"
            } else {
                "没有在这个歌单里找到匹配的歌曲。"
            }
        } else {
            "这个歌单还没有歌曲。"
        };
        container(widgets::hint(message))
            .width(Length::Fill)
            .height(Length::Fill)
            .center_x(Length::Fill)
            .center_y(Length::Fill)
            .into()
    } else {
        scrollable(container(rows).padding(Padding {
            top: 4.0,
            right: 18.0,
            bottom: 16.0,
            left: 18.0,
        }))
        .id(iced::widget::scrollable::Id::new(PLAYLIST_SCROLL_ID))
        .height(Length::Fill)
        .on_scroll(move |viewport| {
            if bili_more && near_bottom(viewport) {
                Message::LoadMoreBiliFolder
            } else {
                Message::PlaylistScrolled(viewport.absolute_offset().y)
            }
        })
        .style(theme::scrollable_auto(
            app.scroll_opacity(ScrollArea::TrackList),
        ))
        .into()
    };

    let search = playlist_search_bar(app, state, detail, total);

    // 滚动放在卡片内部：卡片本身固定，圆角不会随内容滚动被裁成直角。
    let card = container(column![
        header,
        container(search).padding(Padding::from([10, 18])),
        widgets::soft_rule(),
        list
    ])
    .width(Length::Fill)
    .height(Length::Fill)
    .style(theme::card);

    let total_count = detail.track_count.max(detail.tracks.len() as u64);
    let mut content = column![card].spacing(8).height(Length::Fill);

    if !state.local_liked && !state.local_downloads && state.filling {
        content = content.push(widgets::hint(format!(
            "正在加载剩余歌曲… 已显示 {} / 共 {} 首。",
            detail.tracks.len(),
            total_count
        )));
    } else if !state.local_liked
        && !state.local_downloads
        && !detail.track_ids.is_empty()
        && (detail.tracks.len() as u64) < total_count
    {
        // 没有 trackIds 时无法判断是不是真的缺数据（演示数据就没有），不瞎提示。
        content = content.push(widgets::hint(format!(
            "已获取 {} / 共 {} 首（部分歌曲接口没有返回）。",
            detail.tracks.len(),
            total_count
        )));
    }

    content.into()
}

/// 歌单内搜索栏：输入即过滤，右侧按钮只负责重新拉取未加载的歌曲。
fn playlist_search_bar<'a>(
    app: &'a App,
    state: &'a PlaylistState,
    detail: &'a PlaylistDetail,
    matched: usize,
) -> Element<'a> {
    let keyword = !state.search.trim().is_empty();

    let input = text_input("在歌单内搜索歌曲、歌手或专辑…", state.search.as_str())
        .on_input(Message::PlaylistSearchChanged)
        .on_submit(Message::PlaylistSearchSubmit)
        .padding(9)
        .size(12)
        .width(Length::Fill)
        .style(theme::input);

    let submit = button(widgets::svg_icon(
        widgets::ICON_SEARCH,
        14.0,
        iced::Color::WHITE,
    ))
    .on_press(Message::PlaylistSearchSubmit)
    .padding(Padding::from([9, 12]))
    .style(theme::primary);

    let mut bar = row![input, submit].spacing(8).align_y(Alignment::Center);

    if keyword {
        bar = bar.push(widgets::pill(format!("{matched} 首"), PillKind::Muted));
        bar = bar.push(
            button(text("清除").size(11).color(theme::DIM))
                .on_press(Message::PlaylistSearchClear)
                .padding(Padding::from([8, 10]))
                .style(theme::ghost),
        );
    }

    let mut content = column![bar].spacing(6);
    if keyword {
        let loading = state.filling || state.bili_busy;
        let note: Element<'a> = if loading {
            row![
                widgets::spinner(app.phase, 2.5, theme::ACCENT),
                widgets::hint("正在加载剩余歌曲，结果可能会增加…"),
            ]
            .spacing(8)
            .align_y(Alignment::Center)
            .into()
        } else {
            let total = detail.track_count.max(detail.tracks.len() as u64);
            widgets::hint(format!("只在当前歌单的 {total} 首歌曲中查找"))
        };
        content = content.push(note);
    }

    content.into()
}

fn track_header<'a>(downloads: bool) -> Element<'a> {
    row![
        text("#").size(10).color(theme::DIM2).width(26),
        container(row![]).width(38),
        text("歌曲").size(10).color(theme::DIM2).width(Length::Fill),
        text(if downloads { "文件" } else { "专辑" })
            .size(10)
            .color(theme::DIM2)
            .width(180),
        text("时长").size(10).color(theme::DIM2),
        container(row![]).width(19),
        container(row![]).width(19),
        container(row![]).width(22),
    ]
    .spacing(10)
    .padding(Padding::from([6, 6]))
    .into()
}

fn song_row<'a>(
    app: &'a App,
    index: usize,
    song: &'a Song,
    on_press: Message,
    on_enqueue: Message,
    local_downloads: bool,
) -> Element<'a> {
    let active = app.player.current.as_ref().map(|item| item.id) == Some(song.id);
    let title_color = if active { theme::ACCENT } else { theme::FG };

    let marker: Element<'a> = if active {
        widgets::equalizer_live(&app.player.spectrum, 3, 2.0, 14.0, theme::ACCENT)
    } else {
        text(format!("{:02}", index + 1))
            .size(11)
            .color(theme::DIM2)
            .into()
    };

    let title = column![
        text(clip(&song.name, 40)).size(13).color(title_color),
        widgets::artist_link(song, 46, 10.0, theme::DIM),
    ]
    .spacing(2)
    .width(Length::Fill);

    let content = row![
        container(marker)
            .width(26)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center),
        cover(app, &song.cover(96), 38.0, &song.name),
        title,
        file_column(app, song, local_downloads),
        text(song.duration()).size(10).color(theme::DIM2),
        widgets::download_button(app, song, 13.0, local_downloads),
        widgets::like_button(
            app.is_liked(song.id),
            13.0,
            theme::DIM2,
            Message::ToggleLike(song.clone()),
        ),
        mouse_area(
            container(text("＋").size(13).color(theme::DIM2))
                .width(22)
                .align_x(Alignment::Center)
                .align_y(Alignment::Center),
        )
        .on_press(on_enqueue)
        .interaction(mouse::Interaction::Pointer),
    ]
    .spacing(10)
    .padding(Padding::from([6, 6]))
    .align_y(Alignment::Center);

    button(content)
        .on_press(on_press)
        .padding(0)
        .width(Length::Fill)
        .style(theme::row_active(active))
        .into()
}

/// 专辑列：普通列表显示专辑名；本地音乐页显示音质 / 格式与文件大小。
fn file_column<'a>(app: &App, song: &Song, downloads: bool) -> Element<'a> {
    if downloads {
        let label = app
            .download_meta(song.id)
            .map(|(quality, size)| format!("{quality} · {}", format_bytes(size)))
            .or_else(|| {
                app.local_meta(song.id)
                    .map(|(format, size)| format!("{format} · {}", format_bytes(size)))
            })
            .unwrap_or_else(|| "—".to_string());
        text(label).size(10).color(theme::DIM2).width(180).into()
    } else {
        text(clip(song.album_name(), 20))
            .size(10)
            .color(theme::DIM2)
            .width(180)
            .into()
    }
}

fn cover<'a>(app: &App, url: &str, size: f32, label: &str) -> Element<'a> {
    if let Some(handle) = app.covers.get(url) {
        // 缓存里的封面已经居中裁成正方形，`Fill` 与 `Cover` 渲染结果完全一致。
        // 这里不能用 `Cover`：iced 0.13 的 wgpu 渲染器在这种「图片超出控件边界」
        // 的情况下会为每张图额外开一个裁剪图层，滚动区里（尤其是改变窗口宽度
        // 之后）偶发把某张封面画到滚动前的位置，导致封面越到固定标题栏上。
        image(handle.clone())
            .width(size)
            .height(size)
            .content_fit(ContentFit::Fill)
            .into()
    } else {
        let initial = label
            .chars()
            .next()
            .map(|character| character.to_uppercase().to_string())
            .unwrap_or_else(|| "♪".to_string());

        container(text(initial).size(size * 0.34).color(theme::WHITE_82))
            .width(size)
            .height(size)
            .align_x(Alignment::Center)
            .align_y(Alignment::Center)
            .style(|_| container::Style {
                background: Some(theme::brand_gradient()),
                border: border::rounded(8.0),
                ..container::Style::default()
            })
            .into()
    }
}

fn link<'a>(label: &'a str, color: iced::Color, message: Message) -> Element<'a> {
    mouse_area(text(label).size(11).color(color))
        .on_press(message)
        .interaction(mouse::Interaction::Pointer)
        .into()
}

fn clip(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let mut output: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        output.push('…');
    }
    output
}

fn compact(value: u64) -> String {
    if value >= 100_000_000 {
        format!("{:.1} 亿", value as f64 / 100_000_000.0)
    } else if value >= 10_000 {
        format!("{:.1} 万", value as f64 / 10_000.0)
    } else {
        value.to_string()
    }
}

/// 把家目录缩成 `~`，本地音乐的保存路径才放得进描述行。
fn compact_home_path(path: &std::path::Path) -> String {
    let full = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && full.starts_with(&home) => {
            format!("~{}", &full[home.len()..])
        }
        _ => full,
    }
}

/// 本地收藏导入 / 导出弹窗：居中的卡片 + 文件路径输入。
pub(crate) fn liked_transfer_dialog(app: &App) -> Element<'_> {
    let Some(transfer) = &app.liked_transfer else {
        return row![].into();
    };
    let kind = transfer.kind;

    let input = text_input("例如 ~/liked-export.json", transfer.path.as_str())
        .on_input(Message::LikedTransferPath)
        .on_submit(Message::ConfirmLikedTransfer)
        .padding(11)
        .size(12)
        .width(Length::Fill)
        .style(theme::input);

    let mut body = column![
        row![
            text(kind.title()).size(16).color(theme::FG),
            horizontal_space(),
            mouse_area(text("关闭").size(11).color(theme::DIM))
                .on_press(Message::CloseLikedTransfer)
                .interaction(mouse::Interaction::Pointer),
        ]
        .align_y(Alignment::Center),
        text(kind.hint()).size(11).color(theme::DIM2),
        widgets::gradient_rule(2.0),
        input,
    ]
    .spacing(12)
    .width(Length::Fill);

    if let Some(error) = &transfer.error {
        body = body.push(widgets::error_text(error.clone()));
    }

    body = body.push(widgets::hint(
        "支持 {\"songs\": [...]} 与歌曲数组；路径可写 ~/xxx.json",
    ));

    let cancel = button(text("取消").size(12).color(theme::DIM))
        .on_press(Message::CloseLikedTransfer)
        .padding(Padding::from([9, 16]))
        .style(theme::ghost);
    let confirm = button(text(kind.action()).size(12).color(iced::Color::WHITE))
        .on_press(Message::ConfirmLikedTransfer)
        .padding(Padding::from([9, 20]))
        .style(theme::primary);

    body = body.push(
        row![horizontal_space(), cancel, confirm]
            .spacing(8)
            .align_y(Alignment::Center),
    );

    let card = container(body)
        .width(Length::Fixed(480.0))
        .padding(20)
        .style(theme::dialog_panel);

    let backdrop = mouse_area(
        container(row![])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::veil(0.26)),
    )
    .on_press(Message::CloseLikedTransfer);

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

/// 添加 / 管理本地音乐文件夹的弹窗。
pub(crate) fn local_folders_dialog(app: &App) -> Element<'_> {
    let Some(dialog) = &app.local_folder_dialog else {
        return row![].into();
    };

    let mut list = column![].spacing(6);
    if app.local_folders.is_empty() {
        list = list.push(widgets::hint("还没有添加文件夹，可以在下面输入一个目录。"));
    }
    for (index, folder) in app.local_folders.iter().enumerate() {
        let subtitle: Element<'static> = if folder.scanning {
            text("扫描中…").size(10).color(theme::WARN).into()
        } else if let Some(error) = &folder.error {
            text(clip(error, 40)).size(10).color(theme::ERR).into()
        } else {
            text(format!("{} 首", folder.files.len()))
                .size(10)
                .color(theme::DIM2)
                .into()
        };

        let path = compact_home_path(&folder.path);
        list = list.push(
            container(
                row![
                    widgets::svg_icon(widgets::ICON_FOLDER, 15.0, theme::ACCENT),
                    column![text(clip(&path, 52)).size(12).color(theme::FG), subtitle,]
                        .spacing(2)
                        .width(Length::Fill),
                    mouse_area(text("移除").size(11).color(theme::ERR))
                        .on_press(Message::RemoveLocalFolder(index))
                        .interaction(mouse::Interaction::Pointer),
                ]
                .spacing(10)
                .align_y(Alignment::Center),
            )
            .width(Length::Fill)
            .padding(Padding::from([7, 10]))
            .style(|_| container::Style {
                background: Some(Background::Color(theme::SUNK)),
                border: border::rounded(9.0),
                ..container::Style::default()
            }),
        );
    }

    let list_height = (app.local_folders.len().max(1) as f32 * 46.0).clamp(52.0, 168.0);
    let list = scrollable(
        container(list)
            .width(Length::Fill)
            .padding(Padding::from([2, 2])),
    )
    .height(Length::Fixed(list_height))
    .style(theme::scrollable_hidden);

    let input = text_input("例如 ~/Music 或 /mnt/data/Albums", dialog.path.as_str())
        .on_input(Message::LocalFolderPath)
        .on_submit(Message::AddLocalFolder)
        .padding(11)
        .size(12)
        .width(Length::Fill)
        .style(theme::input);

    let add = button(text("添加").size(12).color(iced::Color::WHITE))
        .on_press(Message::AddLocalFolder)
        .padding(Padding::from([9, 20]))
        .style(theme::primary);
    let close = button(text("关闭").size(12).color(theme::DIM))
        .on_press(Message::CloseLocalFolders)
        .padding(Padding::from([9, 16]))
        .style(theme::ghost);
    let rescan = button(text("全部重新扫描").size(12).color(theme::DIM))
        .on_press(Message::RescanLocalFolders)
        .padding(Padding::from([9, 14]))
        .style(theme::ghost);

    let mut body = column![
        row![
            text("本地音乐文件夹").size(16).color(theme::FG),
            horizontal_space(),
            mouse_area(text("关闭").size(11).color(theme::DIM))
                .on_press(Message::CloseLocalFolders)
                .interaction(mouse::Interaction::Pointer),
        ]
        .align_y(Alignment::Center),
        text("添加存放音频的目录，会递归扫描子目录。支持 mp3 / flac / wav / ogg / m4a / aac。")
            .size(11)
            .color(theme::DIM2),
        widgets::gradient_rule(2.0),
        list,
        row![input, add].spacing(8).align_y(Alignment::Center),
    ]
    .spacing(12)
    .width(Length::Fill);

    if let Some(error) = &dialog.error {
        body = body.push(widgets::error_text(error.clone()));
    }

    body = body.push(widgets::hint(
        "移除文件夹不会删除磁盘上的文件；封面优先用内嵌图，其次同目录的 cover.jpg / folder.jpg。",
    ));
    body = body.push(
        row![horizontal_space(), rescan, close]
            .spacing(8)
            .align_y(Alignment::Center),
    );

    let card = container(body)
        .width(Length::Fixed(560.0))
        .padding(20)
        .style(theme::dialog_panel);

    let backdrop = mouse_area(
        container(row![])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::veil(0.26)),
    )
    .on_press(Message::CloseLocalFolders);

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

/// 「清空本地音乐」确认弹窗：不可撤销，需要用户再确认一次。
pub(crate) fn clear_downloads_dialog(app: &App) -> Element<'_> {
    let count = app.download_tracks.len();
    let usage = format_bytes(app.downloads_usage);

    let cancel = button(text("取消").size(12).color(theme::DIM))
        .on_press(Message::CancelClearDownloads)
        .padding(Padding::from([9, 16]))
        .style(theme::ghost);
    let confirm = button(text("清空").size(12).color(iced::Color::WHITE))
        .on_press(Message::ClearDownloads)
        .padding(Padding::from([9, 20]))
        .style(theme::danger);

    let body = column![
        row![
            text("清空已下载歌曲").size(16).color(theme::FG),
            horizontal_space(),
            mouse_area(text("关闭").size(11).color(theme::DIM))
                .on_press(Message::CancelClearDownloads)
                .interaction(mouse::Interaction::Pointer),
        ]
        .align_y(Alignment::Center),
        text(format!(
            "将删除已下载的 {count} 首歌曲（{usage}），此操作不可撤销。"
        ))
        .size(12)
        .color(theme::DIM),
        widgets::gradient_rule(2.0),
        widgets::hint("音乐文件夹里的文件不会被删除；正在播放的歌曲也不会中断。"),
        row![horizontal_space(), cancel, confirm]
            .spacing(8)
            .align_y(Alignment::Center),
    ]
    .spacing(12)
    .width(Length::Fill);

    let card = container(body)
        .width(Length::Fixed(440.0))
        .padding(20)
        .style(theme::dialog_panel);

    let backdrop = mouse_area(
        container(row![])
            .width(Length::Fill)
            .height(Length::Fill)
            .style(|_| theme::veil(0.26)),
    )
    .on_press(Message::CancelClearDownloads);

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

#[cfg(test)]
mod tests {
    use super::grid_metrics;

    #[test]
    fn columns_grow_with_window_width() {
        assert_eq!(grid_metrics(300.0).0, 1);
        assert_eq!(grid_metrics(674.0).0, 3);
        assert_eq!(grid_metrics(1394.0).0, 6);
        assert_eq!(grid_metrics(1614.0).0, 7);
        // 列数只看宽度：只有一两张卡片时也保持正常尺寸。
        let (columns, card) = grid_metrics(2000.0);
        assert_eq!(columns, 8);
        assert!(card < 300.0, "card too wide: {card}");
    }

    #[test]
    fn cards_fill_the_row_without_overflow() {
        for available in [674.0_f32, 1394.0, 1614.0] {
            let (columns, card) = grid_metrics(available);
            let used = card * columns as f32 + 14.0 * (columns as f32 - 1.0);
            assert!(
                used <= available + 0.5,
                "used {used} > available {available}"
            );
            assert!(
                available - used < 15.0,
                "unused {} px too large",
                available - used
            );
        }
    }
}

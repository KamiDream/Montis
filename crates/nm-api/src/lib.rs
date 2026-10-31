//! 网易云音乐 Web API 客户端。
//!
//! 目前只覆盖“登录”这一步所需的最小接口：
//! * `weapi` 参数加密（AES-128-CBC + RSA）；
//! * 二维码登录（unikey / 轮询 / 授权）；
//! * 登录态校验与用户资料；
//! * Cookie 登录兜底。
//!
//! 所有接口都直接请求 `music.163.com`，不依赖本地 Node 服务。

mod bilibili;
mod client;
mod crypto;
mod models;

pub use bilibili::{
    parse_cookie_input as parse_bilibili_cookie_input, BiliAudio, BiliFolder, BiliFolderPage,
    BiliUser, BilibiliClient, FOLDER_PAGE_SIZE,
};
pub use client::{NeteaseClient, QrLogin, QrStatus, QrVerify, UserStats, VerifyQr, VerifyStatus};
pub use crypto::{weapi, WeapiParams};
pub use models::{
    bilibili_song_id, format_duration, is_bilibili_id, is_bilibili_url, sized_cover, Album, Artist,
    ArtistDetail, Creator, PlaylistDetail, PlaylistSummary, SearchPage, Song, SongUrl, Source,
    BILIBILI_ID_OFFSET,
};
pub use nm_core::{BilibiliSession, Profile, Session};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("网络请求失败：{0}")]
    Http(#[from] reqwest::Error),

    #[error("服务返回了无法解析的内容：{0}")]
    Decode(String),

    #[error("参数加密失败：{0}")]
    Crypto(String),

    #[error("接口返回 HTTP {0}")]
    HttpStatus(u16),

    #[error("接口错误（{code}）：{message}")]
    Api { code: i64, message: String },

    /// 接口正常返回，但这首歌没有可用音源（VIP / 购买 / 下架 / 无版权）。
    /// 调用方可以把这种情况当成「自动跳过下一首」，而不是播放失败。
    #[error("{0}")]
    Unavailable(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ApiError>;

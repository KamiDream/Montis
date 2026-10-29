//! 网易云用户资料。
//!
//! 字段名与 `/weapi/w/nuser/account/get` 返回的 `profile` 对象保持一致（camelCase），
//! 因此同一份结构既可以直接反序列化接口响应，也能原样写进本地会话文件。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(default)]
    pub user_id: u64,
    #[serde(default)]
    pub nickname: String,
    #[serde(default)]
    pub avatar_url: String,
    #[serde(default)]
    pub background_url: String,
    #[serde(default)]
    pub signature: String,
    #[serde(default)]
    pub vip_type: u32,
    #[serde(default)]
    pub level: u32,
    #[serde(default)]
    pub follows: u32,
    #[serde(default)]
    pub followeds: u32,
    #[serde(default)]
    pub playlist_count: u32,
    #[serde(default)]
    pub event_count: u32,
}

impl Profile {
    pub fn is_vip(&self) -> bool {
        self.vip_type > 0
    }

    /// 头像 CDN 支持按尺寸裁剪，避免每次拉一张原图。
    pub fn avatar_with_size(&self, size: u32) -> String {
        if self.avatar_url.is_empty() {
            return String::new();
        }
        let separator = if self.avatar_url.contains('?') {
            '&'
        } else {
            '?'
        };
        format!("{}{separator}param={size}y{size}", self.avatar_url)
    }

    /// 没有头像或加载失败时，用昵称首字兜底。
    pub fn initial(&self) -> String {
        self.nickname
            .chars()
            .next()
            .map(|character| character.to_uppercase().to_string())
            .unwrap_or_else(|| "?".to_string())
    }

    pub fn display_name(&self) -> &str {
        if self.nickname.is_empty() {
            "网易云用户"
        } else {
            self.nickname.as_str()
        }
    }

    pub fn signature_or_default(&self) -> &str {
        if self.signature.trim().is_empty() {
            "这个人很懒，什么都没写。"
        } else {
            self.signature.as_str()
        }
    }
}

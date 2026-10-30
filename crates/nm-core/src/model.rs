//! 网易云用户资料。
//!
//! 字段名与 `/weapi/w/nuser/account/get` 返回的 `profile` 对象保持一致（camelCase），
//! 因此同一份结构既可以直接反序列化接口响应，也能原样写进本地会话文件。

use serde::{Deserialize, Deserializer, Serialize};

/// 网易云会把未填写的资料字段直接返回成 `null`（例如没设置签名的账号），
/// 而 `#[serde(default)]` 只处理字段缺失。这里把 `null` 也当作默认值，
/// 避免一个 `null` 就让整个登录校验失败。
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(default, deserialize_with = "null_as_default")]
    pub user_id: u64,
    #[serde(default, deserialize_with = "null_as_default")]
    pub nickname: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub avatar_url: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub background_url: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub signature: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub vip_type: u32,
    #[serde(default, deserialize_with = "null_as_default")]
    pub level: u32,
    #[serde(default, deserialize_with = "null_as_default")]
    pub follows: u32,
    #[serde(default, deserialize_with = "null_as_default")]
    pub followeds: u32,
    #[serde(default, deserialize_with = "null_as_default")]
    pub playlist_count: u32,
    #[serde(default, deserialize_with = "null_as_default")]
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn profile_tolerates_null_fields() {
        // 真实接口：未设置签名的账号会返回 `"signature": null`，
        // 其余可选字段也可能为 null。
        let profile: Profile = serde_json::from_value(json!({
            "userId": 5171596222u64,
            "nickname": "听凭风引",
            "avatarUrl": "https://p2.music.126.net/avatar.jpg",
            "backgroundUrl": null,
            "signature": null,
            "vipType": 110,
            "description": null
        }))
        .expect("profile with nulls");

        assert_eq!(profile.user_id, 5_171_596_222);
        assert_eq!(profile.nickname, "听凭风引");
        assert!(profile.signature.is_empty());
        assert!(profile.background_url.is_empty());
        assert!(profile.is_vip());
    }

    #[test]
    fn profile_tolerates_missing_fields() {
        let profile: Profile = serde_json::from_value(json!({ "userId": 42 })).expect("partial");
        assert_eq!(profile.user_id, 42);
        assert_eq!(profile.level, 0);
    }
}

//! 会话持久化。
//!
//! 会话文件里只有 Cookie 和公开的用户资料，没有密码。
//! 文件权限在 Unix 上强制为 `0600`，避免同机其他用户直接读取登录票据。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cookies::{self, Cookies};
use crate::model::Profile;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Session {
    #[serde(default)]
    pub cookies: Cookies,
    #[serde(default)]
    pub profile: Option<Profile>,
}

impl Session {
    /// 读取会话；文件不存在不算错误。损坏时会退回空会话并给出提示。
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => {
                return (Self::default(), Some(format!("读取本地会话失败：{error}")));
            }
        };

        match serde_json::from_slice::<Self>(&bytes) {
            Ok(session) => (session, None),
            Err(error) => (
                Self::default(),
                Some(format!("本地会话已损坏，已重新开始：{error}")),
            ),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建配置目录失败：{error}"))?;
        }

        let json =
            serde_json::to_vec_pretty(self).map_err(|error| format!("序列化会话失败：{error}"))?;

        // 先写临时文件再原子替换，避免写到一半断电留下半个 JSON。
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, json).map_err(|error| format!("写入会话失败：{error}"))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
        }

        std::fs::rename(&temporary, path).map_err(|error| format!("替换会话文件失败：{error}"))
    }

    pub fn is_logged_in(&self) -> bool {
        cookies::cookie_value(&self.cookies, "MUSIC_U")
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    }

    pub fn cookie_header(&self) -> String {
        cookies::cookie_header(&self.cookies)
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
        self.profile = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let mut session = Session::default();
        session
            .cookies
            .insert("MUSIC_U".to_string(), "token".to_string());
        session.profile = Some(Profile {
            user_id: 42,
            nickname: "听歌的人".to_string(),
            ..Profile::default()
        });

        let directory =
            std::env::temp_dir().join(format!("melody-session-test-{}", std::process::id()));
        let path = directory.join("session.json");
        session.save(&path).expect("save");

        let (loaded, warning) = Session::load(&path);
        assert!(warning.is_none());
        assert!(loaded.is_logged_in());
        assert_eq!(
            loaded
                .profile
                .as_ref()
                .map(|profile| profile.nickname.as_str()),
            Some("听歌的人")
        );

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let (session, warning) = Session::load(Path::new("/nonexistent/melody/session.json"));
        assert!(warning.is_none());
        assert!(!session.is_logged_in());
    }
}

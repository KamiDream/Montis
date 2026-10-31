//! 哔哩哔哩登录态。
//!
//! 与网易云会话分开保存：两个平台可以各自登录，互不影响。
//! 文件里只有 Cookie（`SESSDATA` 等）和公开的账号资料，没有密码；
//! Unix 下同样强制 `0600`。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::cookies::{self, Cookies};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BilibiliSession {
    #[serde(default)]
    pub cookies: Cookies,
    /// 当前账号的 UID，收藏夹列表接口需要它。
    #[serde(default)]
    pub mid: u64,
    #[serde(default)]
    pub name: String,
    /// 头像地址。
    #[serde(default)]
    pub face: String,
    /// 用户在收藏夹网格里隐藏的收藏夹 ID。
    /// 属于设备级偏好，退出登录后仍然保留。
    #[serde(default)]
    pub hidden_folders: Vec<u64>,
}

impl BilibiliSession {
    /// 读取会话；文件不存在不算错误。损坏时退回空会话并给出提示。
    pub fn load(path: &Path) -> (Self, Option<String>) {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (Self::default(), None);
            }
            Err(error) => {
                return (
                    Self::default(),
                    Some(format!("读取哔哩哔哩登录态失败：{error}")),
                );
            }
        };

        match serde_json::from_slice::<Self>(&bytes) {
            Ok(session) => (session, None),
            Err(error) => (
                Self::default(),
                Some(format!("哔哩哔哩登录态已损坏，已忽略：{error}")),
            ),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建配置目录失败：{error}"))?;
        }

        let json = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("序列化哔哩哔哩登录态失败：{error}"))?;

        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, json)
            .map_err(|error| format!("写入哔哩哔哩登录态失败：{error}"))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o600));
        }

        std::fs::rename(&temporary, path)
            .map_err(|error| format!("替换哔哩哔哩登录态文件失败：{error}"))
    }

    pub fn is_logged_in(&self) -> bool {
        cookies::cookie_value(&self.cookies, "SESSDATA")
            .map(|value| !value.is_empty())
            .unwrap_or(false)
    }

    pub fn cookie_header(&self) -> String {
        cookies::cookie_header(&self.cookies)
    }

    pub fn is_folder_hidden(&self, id: u64) -> bool {
        self.hidden_folders.contains(&id)
    }

    /// 切换收藏夹的隐藏状态，返回切换后是否处于隐藏状态。
    pub fn toggle_folder_hidden(&mut self, id: u64) -> bool {
        if let Some(index) = self.hidden_folders.iter().position(|item| *item == id) {
            self.hidden_folders.remove(index);
            false
        } else {
            self.hidden_folders.push(id);
            true
        }
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
        self.mid = 0;
        self.name.clear();
        self.face.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_disk() {
        let mut session = BilibiliSession::default();
        session
            .cookies
            .insert("SESSDATA".to_string(), "token".to_string());
        session.mid = 42;
        session.name = "听歌的人".to_string();
        session.hidden_folders.push(998877);

        let directory =
            std::env::temp_dir().join(format!("montis-bili-session-{}", std::process::id()));
        let path = directory.join("bilibili.json");
        session.save(&path).expect("save");

        let (loaded, warning) = BilibiliSession::load(&path);
        assert!(warning.is_none());
        assert!(loaded.is_logged_in());
        assert_eq!(loaded.mid, 42);
        assert_eq!(loaded.name, "听歌的人");
        assert!(loaded.is_folder_hidden(998877));
        assert!(!loaded.is_folder_hidden(1));

        let _ = std::fs::remove_dir_all(directory);
    }

    #[test]
    fn missing_file_is_not_an_error() {
        let (session, warning) = BilibiliSession::load(Path::new("/nonexistent/melody/bili.json"));
        assert!(warning.is_none());
        assert!(!session.is_logged_in());
    }
}

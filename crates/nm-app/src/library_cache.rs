//! 首页歌单快照：把上次加载到的歌单列表写到 `~/.config/melody/library.json`。
//!
//! 启动时先用它把侧边栏与首页卡片摆出来（封面直接读本地缓存），
//! 网络刷新在后台进行，避免首帧只看到一堆文字占位。
//! 快照里带用户 ID：换账号后旧快照会被忽略，不会串号。

use std::path::Path;

use serde::{Deserialize, Serialize};

use nm_api::PlaylistSummary;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibrarySnapshot {
    /// 上次写入快照的账号，用于登录后校验。
    #[serde(default)]
    pub user_id: u64,
    #[serde(default)]
    pub playlists: Vec<PlaylistSummary>,
}

impl LibrarySnapshot {
    /// 读取快照；文件缺失、损坏或没有歌单时返回 `None`。
    pub fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let snapshot: Self = serde_json::from_str(&text).ok()?;
        (!snapshot.playlists.is_empty()).then_some(snapshot)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("序列化歌单快照失败：{error}"))?;
        std::fs::write(path, text).map_err(|error| format!("保存歌单快照失败：{error}"))
    }

    /// 快照是否属于这个账号。
    pub fn belongs_to(&self, user_id: u64) -> bool {
        self.user_id != 0 && self.user_id == user_id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("montis-library-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn playlist(id: u64, name: &str) -> PlaylistSummary {
        PlaylistSummary {
            id,
            name: name.to_string(),
            cover_img_url: format!("https://p1.music.126.net/{id}.jpg"),
            track_count: 12,
            play_count: 345,
            ..PlaylistSummary::default()
        }
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("library.json");

        let snapshot = LibrarySnapshot {
            user_id: 42,
            playlists: vec![playlist(1, "我喜欢的音乐"), playlist(2, "深夜")],
        };
        snapshot.save(&path).unwrap();

        let loaded = LibrarySnapshot::load(&path).unwrap();
        assert_eq!(loaded.user_id, 42);
        assert_eq!(loaded.playlists.len(), 2);
        assert_eq!(loaded.playlists[0].name, "我喜欢的音乐");
        assert_eq!(
            loaded.playlists[1].cover_img_url,
            "https://p1.music.126.net/2.jpg"
        );
        assert_eq!(loaded.playlists[1].track_count, 12);
        assert!(loaded.belongs_to(42));
        assert!(!loaded.belongs_to(7));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_empty_snapshots_are_ignored() {
        let dir = temp_dir("empty");
        let path = dir.join("library.json");

        assert!(LibrarySnapshot::load(&path).is_none());

        LibrarySnapshot {
            user_id: 1,
            playlists: Vec::new(),
        }
        .save(&path)
        .unwrap();
        assert!(LibrarySnapshot::load(&path).is_none());

        std::fs::write(&path, "{ not json").unwrap();
        assert!(LibrarySnapshot::load(&path).is_none());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_owner_does_not_match() {
        let snapshot = LibrarySnapshot {
            user_id: 0,
            playlists: vec![playlist(1, "x")],
        };
        assert!(!snapshot.belongs_to(0));
        assert!(!snapshot.belongs_to(1));
    }
}

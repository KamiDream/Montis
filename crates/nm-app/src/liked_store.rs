//! 本地「我喜欢的音乐」。
//!
//! 收藏只写本地 `liked.json`，不调用任何云端写接口：
//! 每首歌保存完整的 `Song`，离线、换账号、云端风控都不影响这份列表。

use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use nm_api::{PlaylistDetail, Song};

/// 导入结果：新增了多少首、跳过了多少首（已存在或没有 ID）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MergeReport {
    pub added: usize,
    pub skipped: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LikedStore {
    /// 按收藏时间倒序：最新收藏的在最前面。
    #[serde(default)]
    pub songs: Vec<Song>,
}

impl LikedStore {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("序列化本地收藏失败：{error}"))?;
        std::fs::write(path, text).map_err(|error| format!("保存本地收藏失败：{error}"))
    }

    pub fn len(&self) -> usize {
        self.songs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.songs.is_empty()
    }

    pub fn contains(&self, id: u64) -> bool {
        self.songs.iter().any(|song| song.id == id)
    }

    /// 导出到任意文件：与内部 `liked.json` 同格式，目录不存在时自动创建。
    pub fn export_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建导出目录失败：{error}"))?;
            }
        }
        self.save(path)
    }

    /// 从文件导入并合并到现有收藏。
    ///
    /// 兼容 `{ "songs": [...] }`（Montis 导出的格式）与裸歌曲数组两种写法；
    /// 已收藏的歌曲会保留原位置，新歌按文件里的顺序插到最前面。
    pub fn import_from(&mut self, path: &Path) -> Result<MergeReport, String> {
        let text =
            std::fs::read_to_string(path).map_err(|error| format!("读取文件失败：{error}"))?;
        let songs = if let Ok(store) = serde_json::from_str::<LikedStore>(&text) {
            store.songs
        } else {
            serde_json::from_str::<Vec<Song>>(&text)
                .map_err(|error| format!("无法解析收藏文件：{error}"))?
        };
        Ok(self.merge(songs))
    }

    /// 合并一批歌曲；返回新增与跳过的数量。
    pub fn merge(&mut self, songs: Vec<Song>) -> MergeReport {
        let mut seen: HashSet<u64> = self.songs.iter().map(|song| song.id).collect();
        let mut fresh = Vec::new();
        let mut skipped = 0;
        for song in songs {
            if song.id == 0 || !seen.insert(song.id) {
                skipped += 1;
                continue;
            }
            fresh.push(song);
        }

        let added = fresh.len();
        fresh.extend(std::mem::take(&mut self.songs));
        self.songs = fresh;

        MergeReport { added, skipped }
    }

    /// 切换红心，返回切换后的状态（true = 已收藏）。
    pub fn toggle(&mut self, song: Song) -> bool {
        if let Some(index) = self.songs.iter().position(|item| item.id == song.id) {
            self.songs.remove(index);
            false
        } else {
            self.songs.insert(0, song);
            true
        }
    }

    /// 收藏页封面：用最新收藏那首歌的专辑图。
    pub fn cover_url(&self) -> String {
        self.songs
            .first()
            .and_then(|song| song.al.as_ref())
            .map(|album| album.pic_url.clone())
            .unwrap_or_default()
    }

    /// 把本地收藏包装成歌单详情，复用歌单页的虚拟化列表与播放逻辑。
    pub fn as_playlist(&self) -> PlaylistDetail {
        PlaylistDetail {
            id: 0,
            name: "我喜欢的音乐".to_string(),
            cover_img_url: self.cover_url(),
            track_count: self.len() as u64,
            play_count: 0,
            description: Some("点击歌曲行右侧的红心即可加入或移出这份列表。".to_string()),
            creator: None,
            subscribed: false,
            tracks: self.songs.clone(),
            track_ids: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: u64) -> Song {
        Song {
            id,
            name: format!("歌 {id}"),
            ..Song::default()
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("montis-liked-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn toggle_adds_newest_first_and_removes_on_second_click() {
        let mut store = LikedStore::default();
        assert!(store.toggle(song(1)));
        assert!(store.toggle(song(2)));
        assert_eq!(store.songs[0].id, 2);
        assert!(store.contains(1));
        assert_eq!(store.len(), 2);

        assert!(!store.toggle(song(2)));
        assert!(!store.contains(2));
        assert_eq!(store.len(), 1);
        assert_eq!(store.songs[0].id, 1);
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("liked.json");

        let mut store = LikedStore::default();
        store.toggle(song(7));
        store.toggle(song(8));
        store.save(&path).unwrap();

        let loaded = LikedStore::load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.songs[0].id, 8);
        assert_eq!(loaded.songs[1].name, "歌 7");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_or_broken_files_start_empty() {
        let dir = temp_dir("broken");
        let path = dir.join("liked.json");

        assert!(LikedStore::load(&path).is_empty());
        std::fs::write(&path, "{ not json").unwrap();
        assert!(LikedStore::load(&path).is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_then_import_round_trips_into_an_empty_store() {
        let dir = temp_dir("transfer");
        let path = dir.join("nested/backup.json");

        let mut source = LikedStore::default();
        source.toggle(song(3));
        source.toggle(song(4));
        source.export_to(&path).unwrap();

        let mut target = LikedStore::default();
        let report = target.import_from(&path).unwrap();
        assert_eq!(
            report,
            MergeReport {
                added: 2,
                skipped: 0
            }
        );
        // 文件里本来就是最新收藏在最前，导入后顺序保持不变。
        assert_eq!(target.songs[0].id, 4);
        assert_eq!(target.songs[1].id, 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_merges_new_songs_to_the_front_and_skips_duplicates() {
        let dir = temp_dir("merge");
        let path = dir.join("liked.json");

        let mut source = LikedStore::default();
        source.toggle(song(2));
        source.toggle(song(1)); // 文件顺序：1, 2
        source.export_to(&path).unwrap();

        let mut target = LikedStore::default();
        target.toggle(song(2)); // 已有 2
        let report = target.import_from(&path).unwrap();

        assert_eq!(
            report,
            MergeReport {
                added: 1,
                skipped: 1
            }
        );
        assert_eq!(
            target.songs.iter().map(|s| s.id).collect::<Vec<_>>(),
            [1, 2]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_accepts_a_bare_song_array() {
        let dir = temp_dir("bare");
        let path = dir.join("liked.json");
        std::fs::write(&path, r#"[{"id":11,"name":"甲"},{"id":12,"name":"乙"}]"#).unwrap();

        let mut store = LikedStore::default();
        let report = store.import_from(&path).unwrap();

        assert_eq!(
            report,
            MergeReport {
                added: 2,
                skipped: 0
            }
        );
        assert_eq!(store.songs[0].id, 11);
        assert_eq!(store.songs[1].id, 12);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_reports_unreadable_or_invalid_files() {
        let dir = temp_dir("invalid");
        let path = dir.join("liked.json");

        assert!(LikedStore::default().import_from(&path).is_err());

        std::fs::write(&path, "{ not json").unwrap();
        assert!(LikedStore::default().import_from(&path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn merge_skips_songs_without_an_id() {
        let mut store = LikedStore::default();
        let report = store.merge(vec![Song::default(), song(5), song(5)]);

        assert_eq!(
            report,
            MergeReport {
                added: 1,
                skipped: 2
            }
        );
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn playlist_view_matches_local_store() {
        let mut store = LikedStore::default();
        assert_eq!(store.as_playlist().track_count, 0);

        store.toggle(Song {
            id: 9,
            name: "晴天".to_string(),
            al: Some(nm_api::Album {
                pic_url: "https://p1.music.126.net/a.jpg".to_string(),
                ..nm_api::Album::default()
            }),
            ..Song::default()
        });

        let detail = store.as_playlist();
        assert_eq!(detail.name, "我喜欢的音乐");
        assert_eq!(detail.track_count, 1);
        assert_eq!(detail.tracks[0].id, 9);
        assert_eq!(detail.cover_img_url, "https://p1.music.126.net/a.jpg");
        assert!(detail.missing_track_ids().is_empty());
    }
}

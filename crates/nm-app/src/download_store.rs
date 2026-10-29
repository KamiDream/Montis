//! 本地音乐库：用户主动下载的歌曲。
//!
//! 与播放缓存（[`crate::audio_cache`]）分开存放：
//!
//! - 目录在数据目录 `~/.local/share/melody/music/`，设置页的「清空缓存」
//!   和缓存上限淘汰都不会碰到它；
//! - 不做 LRU，只有用户主动移除 / 清空才会删除；
//! - 歌曲元数据随音频一起保存，断网时也能在「本地音乐」页列出并播放。
//!
//! 目录结构：
//!
//! ```text
//! 123456.flac    音频字节，扩展名取接口返回的实际格式
//! index.json     元数据索引（歌曲、文件名、大小、音质、下载顺序）
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use nm_api::{PlaylistDetail, Song};

const INDEX_FILE: &str = "index.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// 完整的歌曲信息：离线时列表、播放队列都靠它。
    song: Song,
    /// 相对音乐目录的文件名。
    file: String,
    /// 文件字节数，用来统计占用。
    size: u64,
    /// 音源实际档位（请求档位可能被接口降级）。
    label: String,
    /// 下载顺序：越大越新，列表按它倒序。
    added: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    /// 下载顺序计数器，随索引一起持久化。
    seq: u64,
    entries: BTreeMap<u64, Entry>,
}

/// 本地音乐列表里的一首：歌曲信息 + 文件元数据。
#[derive(Debug, Clone)]
pub struct LocalTrack {
    pub song: Song,
    pub label: String,
    pub size: u64,
}

/// 磁盘上的本地音乐库。所有方法都是同步的，异步任务里用 [`lookup`]。
pub struct DownloadStore {
    dir: PathBuf,
    index: Index,
    total: u64,
}

impl DownloadStore {
    /// 打开（必要时创建）音乐目录，清掉索引里文件已经不见的条目。
    pub fn open(dir: PathBuf) -> Self {
        let _ = fs::create_dir_all(&dir);

        // 上次退出时可能留下写到一半的临时文件，直接清掉。
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                if entry.file_name().to_string_lossy().ends_with(".tmp") {
                    let _ = fs::remove_file(entry.path());
                }
            }
        }

        let mut index = fs::read_to_string(dir.join(INDEX_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<Index>(&text).ok())
            .unwrap_or_default();

        let before = index.entries.len();
        index
            .entries
            .retain(|_, entry| dir.join(&entry.file).is_file());
        let pruned = before - index.entries.len();

        let total = index.entries.values().map(|entry| entry.size).sum();
        let store = Self { dir, index, total };
        if pruned > 0 {
            store.save();
        }
        store
    }

    /// 已占用的磁盘空间（字节）。
    pub fn usage(&self) -> u64 {
        self.total
    }

    /// 已下载的歌曲，最新下载的在前。
    pub fn tracks(&self) -> Vec<LocalTrack> {
        let mut entries: Vec<&Entry> = self.index.entries.values().collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.added));
        entries
            .into_iter()
            .map(|entry| LocalTrack {
                song: entry.song.clone(),
                label: entry.label.clone(),
                size: entry.size,
            })
            .collect()
    }

    /// 命中时返回音频字节与音源标签。文件被手动删改过按未命中处理。
    pub fn lookup(&mut self, id: u64) -> Option<(Vec<u8>, String)> {
        let entry = self.index.entries.get(&id)?.clone();
        let bytes = match fs::read(self.dir.join(&entry.file)) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.remove(id);
                return None;
            }
        };
        if bytes.len() as u64 != entry.size {
            self.remove(id);
            return None;
        }
        Some((bytes, entry.label))
    }

    /// 写入一首歌；同一首歌再次下载会替换旧文件。
    pub fn add(&mut self, song: Song, bytes: &[u8], label: &str, kind: &str) -> Result<(), String> {
        if song.id == 0 {
            return Err("歌曲信息不完整，无法保存。".to_string());
        }
        if bytes.is_empty() {
            return Err("下载到的音频是空的。".to_string());
        }

        let file = format!("{}.{}", song.id, file_extension(kind));
        let path = self.dir.join(&file);
        let temp = self.dir.join(format!("{file}.tmp"));
        fs::write(&temp, bytes).map_err(|error| format!("写入本地音乐失败：{error}"))?;
        fs::rename(&temp, &path).map_err(|error| format!("保存本地音乐失败：{error}"))?;

        self.index.seq = self.index.seq.wrapping_add(1);
        let seq = self.index.seq;
        let size = bytes.len() as u64;

        if let Some(previous) = self.index.entries.insert(
            song.id,
            Entry {
                song,
                file: file.clone(),
                size,
                label: label.to_string(),
                added: seq,
            },
        ) {
            self.total = self.total.saturating_sub(previous.size);
            // 换音质后扩展名可能变化，旧文件要清掉，避免留下孤儿文件。
            if previous.file != file {
                let _ = fs::remove_file(self.dir.join(previous.file));
            }
        }
        self.total += size;

        self.save();
        Ok(())
    }

    /// 删除一首歌的文件与索引。
    pub fn remove(&mut self, id: u64) {
        if let Some(entry) = self.index.entries.remove(&id) {
            let _ = fs::remove_file(self.dir.join(entry.file));
            self.total = self.total.saturating_sub(entry.size);
            self.save();
        }
    }

    /// 清空整个本地音乐库。
    pub fn clear(&mut self) {
        for (_, entry) in std::mem::take(&mut self.index.entries) {
            let _ = fs::remove_file(self.dir.join(&entry.file));
        }
        self.total = 0;
        self.save();
    }

    fn save(&self) {
        let Ok(text) = serde_json::to_string_pretty(&self.index) else {
            return;
        };
        let temp = self.dir.join(format!("{INDEX_FILE}.tmp"));
        if fs::write(&temp, text).is_ok() {
            let _ = fs::rename(&temp, self.dir.join(INDEX_FILE));
        }
    }
}

/// 把缓存好的本地歌曲包装成歌单详情，复用歌单页的虚拟化列表与播放逻辑。
pub fn detail(tracks: &[LocalTrack]) -> PlaylistDetail {
    let songs: Vec<Song> = tracks.iter().map(|track| track.song.clone()).collect();
    PlaylistDetail {
        id: 0,
        name: "本地音乐".to_string(),
        // 用最近下载那首歌的专辑图当封面。
        cover_img_url: songs
            .first()
            .and_then(|song| song.al.as_ref())
            .map(|album| album.pic_url.clone())
            .unwrap_or_default(),
        track_count: songs.len() as u64,
        play_count: 0,
        description: Some("下载到本地的歌曲，断网也能播放。".to_string()),
        creator: None,
        subscribed: false,
        // 本地列表不需要补齐逻辑；留空避免出现「缺少歌曲」提示。
        track_ids: Vec::new(),
        tracks: songs,
    }
}

/// 接口给的文件格式（`mp3` / `flac` …）只保留安全字符。
fn file_extension(kind: &str) -> String {
    let cleaned: String = kind
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .take(5)
        .collect::<String>()
        .to_ascii_lowercase();
    if cleaned.is_empty() {
        "audio".to_string()
    } else {
        cleaned
    }
}

/// 共享句柄上的一次查找（播放任务用）：未命中返回 `None`。
pub fn lookup(store: &Arc<Mutex<DownloadStore>>, id: u64) -> Option<(Vec<u8>, String)> {
    lock(store).lookup(id)
}

fn lock(store: &Arc<Mutex<DownloadStore>>) -> std::sync::MutexGuard<'_, DownloadStore> {
    store.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: u64, name: &str) -> Song {
        Song {
            id,
            name: name.to_string(),
            dt: 200_000,
            ..Song::default()
        }
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("montis-music-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn downloads_survive_a_restart() {
        let dir = temp_dir("roundtrip");
        let mut store = DownloadStore::open(dir.clone());
        store
            .add(song(7, "晴天"), b"flac-bytes", "无损", "flac")
            .unwrap();
        assert_eq!(store.tracks().len(), 1);
        assert_eq!(store.usage(), 10);
        assert!(dir.join("7.flac").is_file());

        let mut reopened = DownloadStore::open(dir.clone());
        assert_eq!(reopened.tracks().len(), 1);
        assert_eq!(
            reopened.lookup(7),
            Some((b"flac-bytes".to_vec(), "无损".to_string()))
        );
        assert_eq!(reopened.tracks()[0].song.name, "晴天");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn newest_downloads_come_first() {
        let dir = temp_dir("order");
        let mut store = DownloadStore::open(dir.clone());
        store.add(song(1, "第一"), b"a", "标准", "mp3").unwrap();
        store.add(song(2, "第二"), b"bb", "较高", "mp3").unwrap();
        store.add(song(3, "第三"), b"ccc", "极高", "mp3").unwrap();

        let ids: Vec<u64> = store.tracks().iter().map(|track| track.song.id).collect();
        assert_eq!(ids, vec![3, 2, 1]);
        assert_eq!(store.usage(), 6);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn downloading_again_replaces_the_old_file() {
        let dir = temp_dir("replace");
        let mut store = DownloadStore::open(dir.clone());
        store.add(song(9, "歌"), b"mp3", "标准", "mp3").unwrap();
        store
            .add(song(9, "歌"), b"lossless-bytes", "无损", "flac")
            .unwrap();

        assert_eq!(store.tracks().len(), 1);
        assert_eq!(store.usage(), 14);
        assert!(!dir.join("9.mp3").exists());
        assert!(dir.join("9.flac").is_file());
        assert_eq!(store.lookup(9).unwrap().1, "无损");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn removing_and_clearing_deletes_files() {
        let dir = temp_dir("remove");
        let mut store = DownloadStore::open(dir.clone());
        store.add(song(1, "一"), b"a", "标准", "mp3").unwrap();
        store.add(song(2, "二"), b"bb", "标准", "mp3").unwrap();

        store.remove(1);
        assert!(store.tracks().iter().all(|track| track.song.id != 1));
        assert!(!dir.join("1.mp3").exists());
        assert_eq!(store.usage(), 2);

        store.clear();
        assert!(store.tracks().is_empty());
        assert_eq!(store.usage(), 0);
        assert!(!dir.join("2.mp3").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_temp_files_are_cleaned_on_open() {
        let dir = temp_dir("temp");
        let stale = dir.join("1.flac.tmp");
        fs::write(&stale, b"half").unwrap();

        let store = DownloadStore::open(dir.clone());
        assert!(store.tracks().is_empty());
        assert!(!stale.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_files_are_forgotten_on_open() {
        let dir = temp_dir("missing");
        let mut store = DownloadStore::open(dir.clone());
        store.add(song(1, "一"), b"a", "标准", "mp3").unwrap();
        fs::remove_file(dir.join("1.mp3")).unwrap();

        let store = DownloadStore::open(dir.clone());
        assert!(store.tracks().is_empty());
        assert_eq!(store.usage(), 0);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_truncated_file_is_treated_as_missing() {
        let dir = temp_dir("truncated");
        let mut store = DownloadStore::open(dir.clone());
        store.add(song(1, "一"), b"abcdef", "标准", "mp3").unwrap();
        fs::write(dir.join("1.mp3"), b"abc").unwrap();

        assert!(store.lookup(1).is_none());
        assert!(store.tracks().is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn detail_mirrors_the_local_library() {
        let dir = temp_dir("detail");
        let mut store = DownloadStore::open(dir.clone());
        let mut newest = song(1, "第一");
        newest.al = Some(nm_api::Album {
            pic_url: "https://example.com/pic.jpg".to_string(),
            ..nm_api::Album::default()
        });
        store.add(song(2, "第二"), b"bb", "极高", "mp3").unwrap();
        store.add(newest, b"a", "标准", "mp3").unwrap();

        let detail = detail(&store.tracks());
        assert_eq!(detail.name, "本地音乐");
        assert_eq!(detail.track_count, 2);
        assert_eq!(detail.tracks[0].id, 1);
        assert_eq!(detail.cover_img_url, "https://example.com/pic.jpg");
        // 本地列表不需要补齐：track_ids 保持为空。
        assert!(detail.track_ids.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_extensions_are_sanitized() {
        assert_eq!(file_extension("mp3"), "mp3");
        assert_eq!(file_extension("FLAC"), "flac");
        assert_eq!(file_extension("../evil"), "evil");
        assert_eq!(file_extension(""), "audio");
    }
}

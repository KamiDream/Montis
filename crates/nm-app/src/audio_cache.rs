//! 音频缓存：下载过的歌曲按「歌曲 ID + 音质档位」落盘，
//! 下次播放同一首直接读本地文件，不再走网络。
//!
//! 目录结构（`~/.cache/melody/audio/`）：
//!
//! ```text
//! 7-exhigh.audio   一首歌的完整音频字节（扩展名只用于区分文件）
//! index.json       条目索引：大小、音源标签、最近使用序号
//! ```
//!
//! 索引里记录一个单调递增的 `seq`，每次读取或写入都把条目刷新到最新，
//! 超过上限时从序号最小（最久没听过）的文件开始淘汰；`seq` 会一起持久化，
//! 所以重启之后的最久未用顺序依然准确。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

/// 1 MB 的字节数。
pub const MB: u64 = 1024 * 1024;

/// 缓存上限的最小值（3 GB）：低于这个数存不下几张无损专辑。
pub const LIMIT_MIN_MB: u64 = 3 * 1024;

/// 缓存上限的最大值（10 GB）。
pub const LIMIT_MAX_MB: u64 = 10 * 1024;

/// 滑杆的步进：1 GB。
pub const LIMIT_STEP_MB: u64 = 1024;

/// 默认上限：取最小档，新用户不会一上来就占很多磁盘。
pub const DEFAULT_LIMIT_MB: u64 = LIMIT_MIN_MB;

const INDEX_FILE: &str = "index.json";

/// 缓存键：同一首歌的不同音质分开存。
pub fn key(id: u64, level: &str) -> String {
    format!("{id}-{level}")
}

/// 把字节数写成便于阅读的单位（设置页展示用）。
pub fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MBF: f64 = KB * 1024.0;
    const GB: f64 = MBF * 1024.0;

    fn scaled(value: f64, unit: &str) -> String {
        // 整数就不带小数位（“3 GB”比“3.00 GB”好读）。
        if (value - value.round()).abs() < 0.005 {
            format!("{:.0} {unit}", value)
        } else {
            format!("{:.2} {unit}", value)
        }
    }

    let value = bytes as f64;
    if value >= GB {
        scaled(value / GB, "GB")
    } else if value >= MBF {
        scaled(value / MBF, "MB")
    } else if value >= KB {
        scaled(value / KB, "KB")
    } else {
        format!("{bytes} B")
    }
}

/// 上限（MB）→ 滑杆位置（0~1）。
pub fn limit_to_fraction(limit_mb: u64) -> f32 {
    let span = (LIMIT_MAX_MB - LIMIT_MIN_MB) as f32;
    (limit_mb.saturating_sub(LIMIT_MIN_MB) as f32 / span).clamp(0.0, 1.0)
}

/// 滑杆位置（0~1）→ 上限（MB），按 1 GB 取整。
pub fn fraction_to_limit(fraction: f32) -> u64 {
    let steps = (LIMIT_MAX_MB - LIMIT_MIN_MB) / LIMIT_STEP_MB;
    let step = (fraction.clamp(0.0, 1.0) * steps as f32).round() as u64;
    LIMIT_MIN_MB + step.min(steps) * LIMIT_STEP_MB
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// 相对缓存目录的文件名。
    file: String,
    /// 文件字节数，用来在不解码的情况下统计占用。
    size: u64,
    /// 音源实际档位的中文标签（请求档位可能被接口降级）。
    label: String,
    /// 最近一次使用的序号：越大越新。
    used: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    /// 使用序号计数器，随索引一起持久化。
    seq: u64,
    entries: BTreeMap<String, Entry>,
}

/// 磁盘上的音频缓存。所有方法都是同步的，异步任务里用 [`lookup`] / [`store`]。
pub struct AudioCache {
    dir: PathBuf,
    index: Index,
    limit: u64,
    total: u64,
}

impl AudioCache {
    /// 打开（必要时创建）缓存目录，清掉索引里已经不存在的文件，并把占用压回上限。
    pub fn open(dir: PathBuf, limit: u64) -> Self {
        let _ = fs::create_dir_all(&dir);

        let mut index = fs::read_to_string(dir.join(INDEX_FILE))
            .ok()
            .and_then(|text| serde_json::from_str::<Index>(&text).ok())
            .unwrap_or_default();

        let before = index.entries.len();
        index
            .entries
            .retain(|_, entry| dir.join(&entry.file).is_file());
        let dropped = before - index.entries.len();

        let mut cache = Self {
            total: index.entries.values().map(|entry| entry.size).sum(),
            dir,
            index,
            limit: clamp_limit(limit),
        };
        let pruned = cache.enforce_limit();
        if dropped > 0 || pruned {
            cache.save();
        }
        cache
    }

    /// 当前占用（字节）。
    pub fn usage(&self) -> u64 {
        self.total
    }

    /// 命中时返回音频字节与音源标签，并把它标记为最近使用。
    ///
    /// 文件被手动删改过（长度对不上）时按未命中处理，顺手清掉这条记录。
    pub fn lookup(&mut self, key: &str) -> Option<(Vec<u8>, String)> {
        let entry = self.index.entries.get(key)?.clone();
        let bytes = match fs::read(self.dir.join(&entry.file)) {
            Ok(bytes) => bytes,
            Err(_) => {
                self.remove(key);
                self.save();
                return None;
            }
        };
        if bytes.len() as u64 != entry.size {
            self.remove(key);
            self.save();
            return None;
        }

        let seq = self.tick();
        if let Some(hit) = self.index.entries.get_mut(key) {
            hit.used = seq;
        }
        self.save();
        Some((bytes, entry.label))
    }

    /// 写入缓存；超过上限时淘汰最久没听的文件。缓存关闭时什么也不做。
    pub fn store(&mut self, key: &str, bytes: &[u8], label: &str) -> Result<(), String> {
        // 单个文件就超过上限时存了也会被立刻淘汰，直接跳过。
        if self.limit == 0 || bytes.is_empty() || bytes.len() as u64 > self.limit {
            return Ok(());
        }

        let file = format!("{}.audio", safe_name(key));
        let path = self.dir.join(&file);
        let temp = self.dir.join(format!("{file}.tmp"));
        fs::write(&temp, bytes).map_err(|error| format!("写入音频缓存失败：{error}"))?;
        fs::rename(&temp, &path).map_err(|error| format!("保存音频缓存失败：{error}"))?;

        let seq = self.tick();
        let size = bytes.len() as u64;
        let replaced = self.index.entries.insert(
            key.to_string(),
            Entry {
                file,
                size,
                label: label.to_string(),
                used: seq,
            },
        );
        if let Some(replaced) = replaced {
            self.total = self.total.saturating_sub(replaced.size);
        }
        self.total += size;

        self.enforce_limit();
        self.save();
        Ok(())
    }

    /// 调整上限，并立即把占用压回新的上限（设为 0 会清空缓存）。
    pub fn set_limit(&mut self, limit: u64) {
        self.limit = clamp_limit(limit);
        self.enforce_limit();
        self.save();
    }

    /// 清空所有缓存文件。
    pub fn clear(&mut self) {
        for (_, entry) in std::mem::take(&mut self.index.entries) {
            let _ = fs::remove_file(self.dir.join(&entry.file));
        }
        self.total = 0;
        self.save();
    }

    fn tick(&mut self) -> u64 {
        self.index.seq = self.index.seq.wrapping_add(1);
        self.index.seq
    }

    /// 从最久没用过的条目开始删，直到不超过上限。
    fn enforce_limit(&mut self) -> bool {
        if self.total <= self.limit {
            return false;
        }

        let mut order: Vec<(u64, String)> = self
            .index
            .entries
            .iter()
            .map(|(key, entry)| (entry.used, key.clone()))
            .collect();
        order.sort();

        let mut pruned = false;
        for (_, key) in order {
            if self.total <= self.limit {
                break;
            }
            self.remove(&key);
            pruned = true;
        }
        pruned
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.index.entries.remove(key) {
            let _ = fs::remove_file(self.dir.join(&entry.file));
            self.total = self.total.saturating_sub(entry.size);
        }
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

/// 文件名只保留安全字符，避免键里混进路径分隔符。
fn safe_name(key: &str) -> String {
    key.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

/// 0 表示关闭缓存；其余值夹在 3 GB ~ 10 GB 之间。
fn clamp_limit(limit: u64) -> u64 {
    if limit == 0 {
        0
    } else {
        limit.clamp(LIMIT_MIN_MB * MB, LIMIT_MAX_MB * MB)
    }
}

/// 共享句柄上的一次查找（异步任务用）：未命中返回 `None`。
pub fn lookup(cache: &Arc<Mutex<AudioCache>>, key: &str) -> Option<(Vec<u8>, String)> {
    lock(cache).lookup(key)
}

/// 共享句柄上的一次写入。缓存属于尽力而为，失败不影响播放。
pub fn store(cache: &Arc<Mutex<AudioCache>>, key: &str, bytes: &[u8], label: &str) {
    let _ = lock(cache).store(key, bytes, label);
}

fn lock(cache: &Arc<Mutex<AudioCache>>) -> std::sync::MutexGuard<'_, AudioCache> {
    cache.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("montis-cache-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 真实上限最小 3 GB，测试里没法写这么大的文件：
    /// 直接构造一个小上限的缓存来验证淘汰顺序。
    fn tiny_cache(dir: PathBuf, limit: u64) -> AudioCache {
        let mut cache = AudioCache::open(dir, LIMIT_MIN_MB * MB);
        cache.limit = limit;
        cache
    }

    #[test]
    fn stores_and_survives_a_restart() {
        let dir = temp_dir("roundtrip");
        let mut cache = AudioCache::open(dir.clone(), 8 * MB);
        cache.store("7-exhigh", b"audio-bytes", "极高").unwrap();
        assert_eq!(cache.usage(), 11);
        assert_eq!(
            cache.lookup("7-exhigh"),
            Some((b"audio-bytes".to_vec(), "极高".to_string()))
        );

        let mut reopened = AudioCache::open(dir.clone(), 8 * MB);
        assert_eq!(reopened.usage(), 11);
        assert_eq!(
            reopened.lookup("7-exhigh").map(|(_, label)| label),
            Some("极高".to_string())
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn going_over_the_limit_drops_the_oldest_entry() {
        let dir = temp_dir("evict");
        let mut cache = tiny_cache(dir.clone(), 100);
        cache.store("a", &[1u8; 60], "标准").unwrap();
        cache.store("b", &[2u8; 60], "标准").unwrap();

        // 60 + 60 > 100：先写入的 a 被淘汰，只留下 b。
        assert!(cache.lookup("a").is_none());
        assert!(cache.lookup("b").is_some());
        assert!(cache.usage() <= 100);
        assert!(!dir.join("a.audio").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_a_track_makes_it_the_newest() {
        let dir = temp_dir("touch");
        let mut cache = tiny_cache(dir.clone(), 100);
        cache.store("a", &[1u8; 50], "标准").unwrap();
        cache.store("b", &[2u8; 50], "标准").unwrap();
        // 听一遍 a，它变成最近使用；再存 c 时应该淘汰 b。
        assert!(cache.lookup("a").is_some());
        cache.store("c", &[3u8; 50], "标准").unwrap();

        assert!(cache.lookup("a").is_some());
        assert!(cache.lookup("b").is_none());
        assert!(cache.lookup("c").is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn zero_limit_disables_and_clears_the_cache() {
        let dir = temp_dir("disabled");
        let mut cache = AudioCache::open(dir.clone(), 0);
        cache.store("a", b"xxxx", "标准").unwrap();
        assert_eq!(cache.usage(), 0);
        assert!(cache.lookup("a").is_none());

        let mut cache = AudioCache::open(dir.clone(), 8 * MB);
        cache.store("a", b"xxxx", "标准").unwrap();
        assert_eq!(cache.usage(), 4);
        // 把上限改成 0 等价于清空。
        cache.set_limit(0);
        assert_eq!(cache.usage(), 0);
        assert!(!dir.join("a.audio").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_files_are_forgotten_on_open() {
        let dir = temp_dir("missing");
        let mut cache = AudioCache::open(dir.clone(), 8 * MB);
        cache.store("a", b"xxxx", "标准").unwrap();
        fs::remove_file(dir.join("a.audio")).unwrap();

        let mut reopened = AudioCache::open(dir.clone(), 8 * MB);
        assert_eq!(reopened.usage(), 0);
        assert!(reopened.lookup("a").is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_with_odd_characters_stay_inside_the_directory() {
        assert_eq!(safe_name("12-hires"), "12-hires");
        assert_eq!(safe_name("../x/y"), "___x_y");
    }

    #[test]
    fn bytes_are_formatted_for_the_settings_page() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2 * 1024), "2 KB");
        assert_eq!(format_bytes(300 * MB), "300 MB");
        assert_eq!(format_bytes(3 * 1024 * MB), "3 GB");
        assert_eq!(format_bytes(1536 * MB), "1.50 GB");
    }

    #[test]
    fn the_slider_covers_three_to_ten_gigabytes_in_one_gigabyte_steps() {
        assert_eq!(fraction_to_limit(0.0), LIMIT_MIN_MB);
        assert_eq!(fraction_to_limit(1.0), LIMIT_MAX_MB);
        assert_eq!(fraction_to_limit(0.5), 7 * 1024);
        // 超出两端也不会跑出范围。
        assert_eq!(fraction_to_limit(-1.0), LIMIT_MIN_MB);
        assert_eq!(fraction_to_limit(2.0), LIMIT_MAX_MB);

        assert_eq!(limit_to_fraction(LIMIT_MIN_MB), 0.0);
        assert_eq!(limit_to_fraction(LIMIT_MAX_MB), 1.0);
        // 每一档都能原样走一圈（3 ~ 10 GB，共 8 档）。
        for gb in 3..=10 {
            let limit = gb * 1024;
            assert_eq!(fraction_to_limit(limit_to_fraction(limit)), limit);
        }
    }

    #[test]
    fn the_limit_is_clamped_to_the_supported_range() {
        // 0 表示关闭，不会夹到下限。
        assert_eq!(clamp_limit(0), 0);
        assert_eq!(clamp_limit(512 * MB), LIMIT_MIN_MB * MB);
        assert_eq!(clamp_limit(64 * 1024 * MB), LIMIT_MAX_MB * MB);
    }
}

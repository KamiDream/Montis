//! 封面缓存：下载过的封面按 URL 存到本地，启动时先显示缓存，再在后台刷新一次。
//!
//! 缓存里存的是**原始下载字节**，而不是裁好圆角的像素：
//! 一来同一张图按不同尺寸裁圆时不用重复下载，二来裁圆/圆形封面可以在读取时
//! 重新生成，缓存格式与 `avatar::rounded` 的实现解耦。
//!
//! 目录：`~/.cache/melody/covers/`，文件名是 URL 的哈希，不与 URL 结构耦合。

use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// 最多保留的封面文件数：再多的旧封面按修改时间淘汰（约 100 MB 量级）。
pub const MAX_FILES: usize = 2000;

/// 缓存文件名：URL 的哈希 + 固定扩展名。
pub fn file_name(url: &str) -> String {
    let mut hasher = DefaultHasher::new();
    url.hash(&mut hasher);
    format!("{:016x}.img", hasher.finish())
}

fn file_path(dir: &Path, url: &str) -> PathBuf {
    dir.join(file_name(url))
}

/// 读缓存；没有（或读不出来）时返回 `None`。
pub fn read(dir: &Path, url: &str) -> Option<Vec<u8>> {
    let bytes = fs::read(file_path(dir, url)).ok()?;
    (!bytes.is_empty()).then_some(bytes)
}

/// 写入缓存（临时文件 + rename），失败不影响界面。
pub fn write(dir: &Path, url: &str, bytes: &[u8]) {
    if bytes.is_empty() || fs::create_dir_all(dir).is_err() {
        return;
    }
    let path = file_path(dir, url);
    let temp = path.with_extension("tmp");
    if fs::write(&temp, bytes).is_ok() {
        let _ = fs::rename(&temp, &path);
    }
}

/// 清空目录：删掉所有缓存文件，返回删除的数量。
///
/// 目录本身保留，后续下载可以直接写回去。
pub fn clear(dir: &Path) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };

    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        // 只删普通文件，不碰（理论上不存在的）子目录。
        if path.is_file() && fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 启动时清理：文件数超过 [`MAX_FILES`] 就按修改时间删掉最旧的那些。
///
/// 返回删除的数量（主要给测试用）。
pub fn prune(dir: &Path, max: usize) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };

    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|entry| entry.path().is_file())
        .map(|entry| {
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (modified, entry.path())
        })
        .collect();

    if files.len() <= max {
        return 0;
    }

    // 最旧的排在前面，删到只剩 max 个。
    files.sort_by_key(|(modified, _)| *modified);
    let remove = files.len() - max;
    let mut removed = 0;
    for (_, path) in files.into_iter().take(remove) {
        if fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("montis-covers-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn different_urls_get_different_files() {
        let a = file_name("https://p1.music.126.net/a.jpg?param=96y96");
        let b = file_name("https://p1.music.126.net/a.jpg?param=300y300");
        assert_ne!(a, b);
        assert!(a.ends_with(".img"));
        assert_eq!(a, file_name("https://p1.music.126.net/a.jpg?param=96y96"));
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp_dir("roundtrip");
        let url = "https://p1.music.126.net/a.jpg?param=96y96";

        assert!(read(&dir, url).is_none());
        write(&dir, url, b"png-bytes");
        assert_eq!(read(&dir, url), Some(b"png-bytes".to_vec()));

        // 覆盖写后仍然只有一份文件。
        write(&dir, url, b"newer");
        assert_eq!(read(&dir, url), Some(b"newer".to_vec()));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_images_are_not_cached() {
        let dir = temp_dir("empty");
        write(&dir, "https://example.com/x.jpg", b"");
        assert!(read(&dir, "https://example.com/x.jpg").is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_removes_every_file_but_keeps_the_directory() {
        let dir = temp_dir("clear");
        for index in 0..3 {
            write(&dir, &format!("https://example.com/{index}.jpg"), b"x");
        }

        assert_eq!(clear(&dir), 3);
        assert!(dir.is_dir());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        // 已经空了再清一次也没事。
        assert_eq!(clear(&dir), 0);
        // 缓存可以正常写回来。
        write(&dir, "https://example.com/again.jpg", b"y");
        assert!(read(&dir, "https://example.com/again.jpg").is_some());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pruning_keeps_only_the_newest_files() {
        let dir = temp_dir("prune");
        for index in 0..5 {
            write(&dir, &format!("https://example.com/{index}.jpg"), b"x");
        }
        assert_eq!(prune(&dir, 3), 2);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 3);
        // 不超过上限时什么也不做。
        assert_eq!(prune(&dir, 3), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}

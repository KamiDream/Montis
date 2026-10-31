//! 本地音乐文件夹：用户添加的目录里扫描出来的音频文件。
//!
//! 与「下载」不同，这些文件留在用户自己的目录里，只保存文件夹路径与歌曲索引：
//!
//! - 文件夹列表写在 `~/.config/melody/music-folders.json`；
//! - 每次启动 / 手动刷新时递归扫描一遍，读取标签与时长（读取失败就退回文件名）；
//! - 内嵌封面按需读取，生成好的缩略图仍用封面缓存（`~/.cache/melody/covers/`）；
//! - 移除文件夹只从列表里去掉，不会删除磁盘上的任何文件。
//!
//! 扫描使用 `lofty` 读取 mp3 / flac / m4a / ogg 等格式的标签，不依赖网络。

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
use serde::{Deserialize, Serialize};

use nm_api::{Album, Artist, Song};

/// 本地内嵌封面的伪 URL 前缀：`local-cover://{歌曲 id}?param=96y96`。
///
/// 封面链路（`ensure_covers`）看到这个前缀就改为读文件里的图片，
/// 不会发网络请求。
pub const COVER_PREFIX: &str = "local-cover://";

/// 支持的音频扩展名（小写）。
///
/// 与 rodio 实际能解码的格式保持一致：默认的 flac / vorbis / wav / mp3，
/// 以及工作区额外开启的 AAC 与 MP4 容器。不支持的格式列出来只会播放失败。
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "flac", "wav", "ogg", "oga", "m4a", "aac"];

/// 单个文件夹最多收录多少首，避免误选整个主目录时卡死。
const MAX_FILES_PER_FOLDER: usize = 10_000;
/// 递归深度上限，顺带挡住异常深的目录树。
const MAX_DEPTH: usize = 16;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct FolderIndex {
    folders: Vec<String>,
}

/// 扫描出来的一个音频文件。
#[derive(Debug, Clone)]
pub struct LocalFile {
    pub song: Song,
    pub path: PathBuf,
    /// 文件大小（字节）。
    pub size: u64,
    /// 大写扩展名，例如 `FLAC`。
    pub format: String,
}

/// 一个文件夹的扫描结果。
#[derive(Debug, Clone)]
pub struct FolderScan {
    pub path: PathBuf,
    pub files: Vec<LocalFile>,
    pub error: Option<String>,
}

/// 读取用户保存的文件夹列表；文件缺失或损坏时返回空列表。
pub fn load_folders(path: &Path) -> Vec<PathBuf> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<FolderIndex>(&text).ok())
        .map(|index| {
            index
                .folders
                .into_iter()
                .map(|path| path.trim().to_string())
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

/// 保存文件夹列表（原子写入，避免中途退出留下半个文件）。
pub fn save_folders(path: &Path, folders: &[PathBuf]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|error| format!("创建配置目录失败：{error}"))?;
        }
    }

    let index = FolderIndex {
        folders: folders
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
    };
    let text = serde_json::to_string_pretty(&index)
        .map_err(|error| format!("序列化音乐文件夹失败：{error}"))?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, text).map_err(|error| format!("保存音乐文件夹失败：{error}"))?;
    fs::rename(&temp, path).map_err(|error| format!("写入音乐文件夹失败：{error}"))
}

/// 逐个扫描文件夹；单个文件夹失败不会影响其它文件夹。
pub async fn scan_folders(paths: Vec<PathBuf>) -> Vec<FolderScan> {
    paths.into_iter().map(|path| scan_folder(&path)).collect()
}

/// 递归扫描一个文件夹，返回其中的音频文件。
pub fn scan_folder(root: &Path) -> FolderScan {
    if !root.is_dir() {
        return FolderScan {
            path: root.to_path_buf(),
            files: Vec::new(),
            error: Some("文件夹不存在或无法访问。".to_string()),
        };
    }
    if let Err(error) = fs::read_dir(root) {
        return FolderScan {
            path: root.to_path_buf(),
            files: Vec::new(),
            error: Some(format!("无法读取文件夹：{error}")),
        };
    }

    let mut paths = Vec::new();
    collect_audio(root, 0, &mut paths);
    paths.sort();

    // 同一目录里的 `cover.jpg` / `folder.jpg` 常被当作专辑封面：
    // 每首文件都查一次目录太浪费，按目录缓存结果。
    let mut covers: HashMap<PathBuf, Option<PathBuf>> = HashMap::new();
    let files: Vec<LocalFile> = paths
        .iter()
        .map(|path| {
            let dir = path.parent().unwrap_or(Path::new("")).to_path_buf();
            let folder_cover = covers
                .entry(dir.clone())
                .or_insert_with(|| folder_cover(&dir));
            local_file(path, folder_cover.as_deref())
        })
        .collect();
    FolderScan {
        path: root.to_path_buf(),
        files,
        error: None,
    }
}

/// 读取一首本地歌曲的封面：优先内嵌图片，其次同目录的 `cover.jpg` 等。
pub fn read_cover(path: &Path) -> Option<Vec<u8>> {
    if let Ok(tagged) = lofty::read_from_path(path) {
        if let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) {
            if let Some(picture) = tag.pictures().first() {
                return Some(picture.data().to_vec());
            }
        }
    }

    let dir = path.parent()?;
    let cover = folder_cover(dir)?;
    fs::read(cover).ok()
}

/// 在目录里找常见的封面文件名（不区分大小写）。
fn folder_cover(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(dir).ok()?.flatten().collect();
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let is_file = entry
            .file_type()
            .map(|kind| kind.is_file())
            .unwrap_or(false);
        if !is_file {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "cover.jpg"
                | "cover.jpeg"
                | "cover.png"
                | "folder.jpg"
                | "folder.jpeg"
                | "folder.png"
                | "front.jpg"
                | "front.jpeg"
                | "front.png"
        ) {
            return Some(entry.path());
        }
    }
    None
}

/// 本地歌曲的稳定 id：路径的 FNV-1a 64 位哈希，最高位置 1。
///
/// 最高位用来和网易云的数字 id 区分开（真实歌曲 id 远小于 2^63）；
/// 同一路径跨重启得到同一个 id，播放快照与本地收藏才能对上。
pub fn local_id(path: &Path) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash | (1 << 63)
}

fn collect_audio(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES_PER_FOLDER {
        return;
    }

    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<_> = entries.flatten().collect();
    items.sort_by_key(|entry| entry.file_name());

    for entry in items {
        if out.len() >= MAX_FILES_PER_FOLDER {
            break;
        }

        let name = entry.file_name();
        let name = name.to_string_lossy();
        // 隐藏目录 / 隐藏文件（.thumbnails、.DS_Store 等）不参与扫描。
        if name.starts_with('.') {
            continue;
        }

        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // 不跟随符号链接目录（避免目录环），但允许指向单个音频文件的链接。
        if kind.is_symlink() {
            let path = entry.path();
            if path.is_file() && audio_extension(&path).is_some() {
                out.push(path);
            }
            continue;
        }
        if kind.is_dir() {
            collect_audio(&entry.path(), depth + 1, out);
        } else if kind.is_file() && audio_extension(&entry.path()).is_some() {
            out.push(entry.path());
        }
    }
}

fn audio_extension(path: &Path) -> Option<String> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    AUDIO_EXTENSIONS
        .contains(&extension.as_str())
        .then_some(extension)
}

fn local_file(path: &Path, folder_cover: Option<&Path>) -> LocalFile {
    let metadata = read_metadata(path, folder_cover);
    let size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let format = path
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_uppercase())
        .unwrap_or_else(|| "AUDIO".to_string());

    let id = local_id(path);
    let cover = if metadata.has_cover {
        format!("{COVER_PREFIX}{id}")
    } else {
        String::new()
    };

    let song = Song {
        id,
        name: metadata.title,
        ar: vec![Artist {
            id: 0,
            name: metadata.artist,
        }],
        al: Some(Album {
            id: 0,
            name: metadata.album,
            pic_url: cover,
        }),
        dt: metadata.duration_ms,
        fee: 0,
        local_path: path.to_string_lossy().into_owned(),
        ..Song::default()
    };

    LocalFile {
        song,
        path: path.to_path_buf(),
        size,
        format,
    }
}

struct Metadata {
    title: String,
    artist: String,
    album: String,
    duration_ms: u64,
    has_cover: bool,
}

/// 读取标签；任何一步失败都退回到文件名 / 所在目录名，保证文件仍能被列出。
fn read_metadata(path: &Path, folder_cover: Option<&Path>) -> Metadata {
    let fallback_title = path
        .file_stem()
        .map(|value| value.to_string_lossy().trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "未命名".to_string());
    let fallback_album = path
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|value| value.to_string_lossy().trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "本地音乐".to_string());

    let folder_has_cover = folder_cover.is_some();
    let mut meta = Metadata {
        title: fallback_title,
        artist: "本地文件".to_string(),
        album: fallback_album,
        duration_ms: 0,
        has_cover: folder_has_cover,
    };

    let Ok(tagged) = lofty::read_from_path(path) else {
        return meta;
    };

    meta.duration_ms = tagged.properties().duration().as_millis() as u64;

    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let Some(tag) = tag else {
        return meta;
    };

    if let Some(title) = tag.title().map(|value| value.trim().to_string()) {
        if !title.is_empty() {
            meta.title = title;
        }
    }
    if let Some(artist) = tag.artist().map(|value| value.trim().to_string()) {
        if !artist.is_empty() {
            meta.artist = artist;
        }
    }
    if let Some(album) = tag.album().map(|value| value.trim().to_string()) {
        if !album.is_empty() {
            meta.album = album;
        }
    }
    meta.has_cover = !tag.pictures().is_empty() || folder_has_cover;
    meta
}

/// 去掉重复的歌曲 id（同一个文件被两个嵌套文件夹收录时只保留一次）。
pub fn dedupe(files: Vec<LocalFile>) -> Vec<LocalFile> {
    let mut seen = HashSet::new();
    files
        .into_iter()
        .filter(|file| seen.insert(file.song.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("montis-local-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path, bytes: &[u8]) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, bytes).unwrap();
    }

    /// 创建一个 0.1 秒的静音 WAV，给标签读写测试用。
    fn silent_wav(path: &Path) {
        let sample_rate: u32 = 8_000;
        let samples: u32 = 800;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + samples).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&8u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&samples.to_le_bytes());
        bytes.extend(std::iter::repeat(128u8).take(samples as usize));
        fs::write(path, bytes).unwrap();
    }

    #[test]
    fn scans_nested_folders_and_skips_non_audio() {
        let dir = temp_dir("scan");
        touch(&dir.join("b.flac"), b"not really flac");
        touch(&dir.join("a.mp3"), b"not really mp3");
        touch(&dir.join("notes.txt"), b"text");
        touch(&dir.join(".hidden.mp3"), b"hidden");
        touch(&dir.join("sub/c.wav"), b"wav");
        touch(&dir.join("sub/deep/d.m4a"), b"m4a");

        let scan = scan_folder(&dir);
        assert!(scan.error.is_none());
        let names: Vec<&str> = scan
            .files
            .iter()
            .map(|file| file.song.name.as_str())
            .collect();
        assert_eq!(names, vec!["a", "b", "c", "d"]);
        let formats: Vec<&str> = scan.files.iter().map(|file| file.format.as_str()).collect();
        assert_eq!(formats, vec!["MP3", "FLAC", "WAV", "M4A"]);

        // 标签读不出来时仍然保留本地路径与所在目录。
        assert!(scan.files[0].song.local_path.ends_with("a.mp3"));
        assert_eq!(scan.files[0].song.artist_names(), "本地文件");
        assert!(!scan.files[0].song.al.as_ref().unwrap().name.is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_folder_reports_an_error() {
        let dir = temp_dir("missing");
        let scan = scan_folder(&dir.join("nope"));
        assert!(scan.files.is_empty());
        assert!(scan.error.is_some());
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn follows_symlinked_files_but_not_directory_links() {
        let dir = temp_dir("symlink");
        let real = dir.join("real");
        fs::create_dir_all(&real).unwrap();
        touch(&real.join("a.mp3"), b"x");
        touch(&dir.join("b.mp3"), b"x");
        std::os::unix::fs::symlink(&real, dir.join("link-dir")).unwrap();
        std::os::unix::fs::symlink(real.join("a.mp3"), dir.join("link.mp3")).unwrap();

        let scan = scan_folder(&dir);
        let mut names: Vec<String> = scan
            .files
            .iter()
            .map(|file| {
                file.path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        // a.mp3（真实文件）+ b.mp3 + link.mp3；link-dir 里的 a.mp3 不重复扫描。
        assert_eq!(names, vec!["a.mp3", "b.mp3", "link.mp3"]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_ids_are_stable_and_never_collide_with_online_ids() {
        let path = Path::new("/music/a.flac");
        assert_eq!(local_id(path), local_id(path));
        assert_ne!(local_id(path), local_id(Path::new("/music/b.flac")));
        // 最高位固定为 1，网易云 id 不会落到这个区间。
        assert_ne!(local_id(path) & (1 << 63), 0);
    }

    #[test]
    fn folder_list_round_trips_through_disk() {
        let dir = temp_dir("config");
        let path = dir.join("music-folders.json");
        let folders = vec![PathBuf::from("/a/b"), PathBuf::from("/c d/e")];

        save_folders(&path, &folders).unwrap();
        assert_eq!(load_folders(&path), folders);

        fs::write(&path, "{ broken").unwrap();
        assert!(load_folders(&path).is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn duplicate_files_are_reduced_by_path_id() {
        let file = LocalFile {
            song: Song {
                id: 7,
                ..Song::default()
            },
            path: PathBuf::from("/a.mp3"),
            size: 1,
            format: "MP3".to_string(),
        };
        let mut other = file.clone();
        other.path = PathBuf::from("/sub/a.mp3");

        let files = dedupe(vec![file, other]);
        assert_eq!(files.len(), 1);
    }

    #[test]
    fn reads_tags_from_audio_files() {
        use lofty::config::WriteOptions;
        use lofty::tag::{Accessor as _, Tag, TagExt, TagType};

        let dir = temp_dir("tags");
        let path = dir.join("tagged.wav");
        silent_wav(&path);

        let mut tag = Tag::new(TagType::RiffInfo);
        tag.set_title("标签标题".to_string());
        tag.set_artist("标签歌手".to_string());
        tag.set_album("标签专辑".to_string());
        tag.save_to_path(&path, WriteOptions::default()).unwrap();

        let scan = scan_folder(&dir);
        let file = &scan.files[0];
        assert_eq!(file.song.name, "标签标题");
        assert_eq!(file.song.artist_names(), "标签歌手");
        assert_eq!(file.song.al.as_ref().unwrap().name, "标签专辑");
        assert!(file.song.dt > 0, "时长应该来自音频头");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn uses_a_folder_cover_when_the_file_has_none() {
        let dir = temp_dir("folder-cover");
        touch(&dir.join("cover.jpg"), b"fake-cover");
        touch(&dir.join("song.mp3"), b"x");

        let scan = scan_folder(&dir);
        let file = scan
            .files
            .iter()
            .find(|file| file.song.name == "song")
            .unwrap();
        assert!(file
            .song
            .al
            .as_ref()
            .unwrap()
            .pic_url
            .starts_with(COVER_PREFIX));
        assert_eq!(read_cover(&file.path).unwrap(), b"fake-cover");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn covers_of_untagged_files_are_absent() {
        let dir = temp_dir("cover");
        let file = dir.join("x.mp3");
        touch(&file, b"nothing");
        assert!(read_cover(&file).is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}

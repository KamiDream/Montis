//! 本地目录约定。
//!
//! Linux 下为 `~/.config/melody/`，Windows 为 `%APPDATA%\melody\`，
//! macOS 为 `~/Library/Application Support/melody/`。

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Self {
        let base = dirs::config_dir().unwrap_or_else(|| PathBuf::from("."));
        Self {
            config_dir: base.join("melody"),
        }
    }

    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.config_dir)
    }

    pub fn session_file(&self) -> PathBuf {
        self.config_dir.join("session.json")
    }

    /// 音频可视化（环形频谱）的偏好设置。
    pub fn visualizer_file(&self) -> PathBuf {
        self.config_dir.join("visualizer.json")
    }

    /// 界面偏好（动画速度等）。
    pub fn settings_file(&self) -> PathBuf {
        self.config_dir.join("settings.json")
    }

    /// 播放器快照：关闭应用前的队列、当前歌曲与进度，重启后恢复底部播放条。
    pub fn player_file(&self) -> PathBuf {
        self.config_dir.join("player.json")
    }

    /// 首页歌单快照：上次加载到的歌单列表，重启后先摆出来，再在后台刷新。
    pub fn library_file(&self) -> PathBuf {
        self.config_dir.join("library.json")
    }

    /// 本地红心收藏：只保存收藏的歌曲，不跟云端同步。
    pub fn liked_file(&self) -> PathBuf {
        self.config_dir.join("liked.json")
    }

    /// 本地收藏的导入 / 导出默认文件：与内部 `liked.json` 同格式，方便备份与迁移。
    pub fn liked_transfer_file(&self) -> PathBuf {
        self.config_dir.join("liked-export.json")
    }

    /// 缓存根目录：Linux 下为 `~/.cache/melody/`，不跟配置混在一起。
    pub fn cache_dir(&self) -> PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| self.config_dir.clone())
            .join("melody")
    }

    /// 下载过的歌曲存在这里，重启后仍可复用。
    pub fn audio_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("audio")
    }

    /// 用户主动下载的本地音乐。
    ///
    /// 放在数据目录而不是缓存目录：清空缓存、缓存上限淘汰都不会碰到它，
    /// 下载的内容才会真正长期留在本地。
    /// 本地音乐：用户主动下载的歌曲。
    pub fn music_dir(&self) -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| self.config_dir.clone())
            .join("melody")
            .join("music")
    }

    /// 用户添加的本地音乐文件夹（只保存路径，文件在用户自己的目录里）。
    pub fn music_folders_file(&self) -> PathBuf {
        self.config_dir.join("music-folders.json")
    }

    /// 封面图片缓存：启动时先用它显示，再在后台刷新一次。
    pub fn cover_cache_dir(&self) -> PathBuf {
        self.cache_dir().join("covers")
    }
}

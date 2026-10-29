//! 播放器快照：把关闭前的队列、当前歌曲、进度、模式与音量写到
//! `~/.config/melody/player.json`，重启后恢复底部播放条。

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use nm_api::Song;

use crate::app::{AudioQuality, PlayMode};

/// 默认音量，与 `PlayerState::new` 保持一致。
pub const DEFAULT_VOLUME: f32 = 0.85;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    /// 关闭应用时的播放队列。
    #[serde(default)]
    pub queue: Vec<Song>,
    /// 当前播放项在队列里的下标。
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub mode: PlayMode,
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// 用户选择的音质档位；旧快照没有这个字段时默认极高。
    #[serde(default)]
    pub quality: AudioQuality,
    /// 关闭应用时的播放进度（秒）。
    #[serde(default)]
    pub position: f64,
}

fn default_volume() -> f32 {
    DEFAULT_VOLUME
}

impl Default for PlayerSnapshot {
    fn default() -> Self {
        Self {
            queue: Vec::new(),
            index: 0,
            mode: PlayMode::default(),
            volume: DEFAULT_VOLUME,
            quality: AudioQuality::default(),
            position: 0.0,
        }
    }
}

impl PlayerSnapshot {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        serde_json::from_str(&text).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("序列化播放状态失败：{error}"))?;
        std::fs::write(path, text).map_err(|error| format!("保存播放状态失败：{error}"))
    }

    /// 没有队列可恢复（首次启动或已清空）时，界面不应该出现播放条。
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// 恢复用的进度：负数归零；已知时长时不超过歌曲长度。
    pub fn position(&self, duration: Duration) -> Duration {
        let position = Duration::from_secs_f64(self.position.max(0.0));
        if duration.is_zero() {
            position
        } else {
            position.min(duration)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::RepeatMode;

    fn song(id: u64, name: &str) -> Song {
        Song {
            id,
            name: name.to_string(),
            dt: 180_000,
            ..Song::default()
        }
    }

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("melo-player-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn round_trips_through_disk() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("player.json");

        let snapshot = PlayerSnapshot {
            queue: vec![song(1, "第一首"), song(2, "第二首")],
            index: 1,
            mode: PlayMode {
                shuffle: true,
                repeat: RepeatMode::All,
            },
            volume: 0.4,
            quality: AudioQuality::Lossless,
            position: 42.5,
        };
        snapshot.save(&path).unwrap();

        let loaded = PlayerSnapshot::load(&path);
        assert_eq!(loaded.queue.len(), 2);
        assert_eq!(loaded.queue[0].id, 1);
        assert_eq!(loaded.queue[1].name, "第二首");
        assert_eq!(loaded.index, 1);
        assert_eq!(
            loaded.mode,
            PlayMode {
                shuffle: true,
                repeat: RepeatMode::All,
            }
        );
        assert!((loaded.volume - 0.4).abs() < 1e-6);
        assert_eq!(loaded.quality, AudioQuality::Lossless);
        assert!((loaded.position - 42.5).abs() < 1e-6);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_has_nothing_to_restore() {
        let dir = temp_dir("missing");
        let loaded = PlayerSnapshot::load(&dir.join("player.json"));

        assert!(loaded.is_empty());
        assert_eq!(loaded.mode, PlayMode::default());
        assert!((loaded.volume - DEFAULT_VOLUME).abs() < 1e-6);
        assert_eq!(loaded.quality, AudioQuality::ExHigh);
        assert_eq!(loaded.position, 0.0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_snapshots_fall_back_to_defaults() {
        let dir = temp_dir("partial");
        let path = dir.join("player.json");
        std::fs::write(&path, r#"{"queue": [{"id": 7, "name": "只有队列"}]}"#).unwrap();

        let loaded = PlayerSnapshot::load(&path);
        assert_eq!(loaded.queue.len(), 1);
        assert_eq!(loaded.queue[0].id, 7);
        assert_eq!(loaded.index, 0);
        assert_eq!(loaded.mode, PlayMode::default());
        assert!((loaded.volume - DEFAULT_VOLUME).abs() < 1e-6);
        assert_eq!(loaded.quality, AudioQuality::ExHigh);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn quality_serializes_as_api_level() {
        let dir = temp_dir("quality");
        let path = dir.join("player.json");
        let snapshot = PlayerSnapshot {
            quality: AudioQuality::HiRes,
            ..PlayerSnapshot::default()
        };
        snapshot.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"hires\""));
        assert_eq!(PlayerSnapshot::load(&path).quality, AudioQuality::HiRes);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_snapshot_with_a_single_mode_string_still_loads() {
        let dir = temp_dir("legacy");
        let path = dir.join("player.json");
        std::fs::write(&path, r#"{"queue": [{"id": 7}], "mode": "shuffle"}"#).unwrap();

        let loaded = PlayerSnapshot::load(&path);
        assert_eq!(
            loaded.mode,
            PlayMode {
                shuffle: true,
                repeat: RepeatMode::Off,
            }
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn position_is_clamped_to_the_song_length() {
        let duration = Duration::from_secs(180);

        let within = PlayerSnapshot {
            position: 42.0,
            ..PlayerSnapshot::default()
        };
        assert_eq!(within.position(duration), Duration::from_secs(42));

        let negative = PlayerSnapshot {
            position: -5.0,
            ..PlayerSnapshot::default()
        };
        assert_eq!(negative.position(duration), Duration::ZERO);

        let beyond = PlayerSnapshot {
            position: 999.0,
            ..PlayerSnapshot::default()
        };
        assert_eq!(beyond.position(duration), duration);
    }

    #[test]
    fn position_survives_unknown_duration() {
        let snapshot = PlayerSnapshot {
            position: 30.0,
            ..PlayerSnapshot::default()
        };

        assert_eq!(snapshot.position(Duration::ZERO), Duration::from_secs(30));
    }
}

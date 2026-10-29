//! 界面偏好：与播放、可视化参数无关的 UI 设置。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::audio_cache::{DEFAULT_LIMIT_MB, LIMIT_MAX_MB, LIMIT_MIN_MB};

/// 动画速度预设：倍率与显示名称（慢 / 标准 / 快 / 极快）。
pub const ANIMATION_PRESETS: [(f32, &str); 4] =
    [(0.6, "慢"), (1.0, "标准"), (1.4, "快"), (1.9, "极快")];

/// 界面偏好，保存为 `~/.config/melody/settings.json`。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(default)]
pub struct UiSettings {
    /// 界面动画速度：1.0 为标准，越大越快。
    pub animation_speed: f32,
    /// 正在播放页控制台的不透明度：1.0 为实心白，越低越透。
    pub dock_opacity: f32,
    /// 音频缓存上限（MB）；0 表示不缓存。
    pub cache_limit_mb: u64,
    /// 首次启动的使用须知是否已确认：只保存一次，之后不再弹出。
    pub disclaimer_accepted: bool,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            animation_speed: 1.0,
            dock_opacity: 0.84,
            cache_limit_mb: DEFAULT_LIMIT_MB,
            disclaimer_accepted: false,
        }
    }
}

impl UiSettings {
    pub fn load(path: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let Ok(mut settings) = serde_json::from_str::<Self>(&text) else {
            return Self::default();
        };
        settings.clamp();
        settings
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self)
            .map_err(|error| format!("序列化设置失败：{error}"))?;
        std::fs::write(path, text).map_err(|error| format!("保存设置失败：{error}"))
    }

    pub fn clamp(&mut self) {
        self.animation_speed = self.animation_speed.clamp(0.3, 3.0);
        self.dock_opacity = self
            .dock_opacity
            .clamp(DOCK_OPACITY_RANGE.0, DOCK_OPACITY_RANGE.1);
        self.cache_limit_mb = if self.cache_limit_mb == 0 {
            0
        } else {
            self.cache_limit_mb.clamp(LIMIT_MIN_MB, LIMIT_MAX_MB)
        };
    }
}

/// 控制台透明度的可调范围（滑杆两端）：再低文字对比度会不够。
pub const DOCK_OPACITY_RANGE: (f32, f32) = (0.6, 1.0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_is_clamped() {
        let mut settings = UiSettings {
            animation_speed: 9.0,
            ..UiSettings::default()
        };
        settings.clamp();
        assert_eq!(settings.animation_speed, 3.0);

        settings.animation_speed = 0.0;
        settings.clamp();
        assert_eq!(settings.animation_speed, 0.3);
    }

    #[test]
    fn dock_opacity_is_clamped() {
        let mut settings = UiSettings {
            dock_opacity: 0.0,
            ..UiSettings::default()
        };
        settings.clamp();
        assert!((settings.dock_opacity - 0.6).abs() < f32::EPSILON);

        settings.dock_opacity = 2.0;
        settings.clamp();
        assert!((settings.dock_opacity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn missing_file_falls_back_to_default() {
        let path = std::env::temp_dir().join("montis-missing-ui-settings.json");
        let _ = std::fs::remove_file(&path);
        let settings = UiSettings::load(&path);
        assert_eq!(settings.animation_speed, 1.0);
        assert!((settings.dock_opacity - 0.84).abs() < f32::EPSILON);
        assert_eq!(settings.cache_limit_mb, DEFAULT_LIMIT_MB);
    }

    #[test]
    fn unknown_fields_keep_defaults() {
        let path = std::env::temp_dir().join("montis-partial-ui-settings.json");
        std::fs::write(&path, "{}").expect("write");
        let settings = UiSettings::load(&path);
        assert_eq!(settings.animation_speed, 1.0);
        assert!((settings.dock_opacity - 0.84).abs() < f32::EPSILON);
        assert_eq!(settings.cache_limit_mb, DEFAULT_LIMIT_MB);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn old_settings_without_opacity_keep_the_default() {
        let path = std::env::temp_dir().join("montis-old-ui-settings.json");
        std::fs::write(&path, r#"{"animation_speed": 1.4}"#).expect("write");
        let settings = UiSettings::load(&path);
        assert!((settings.animation_speed - 1.4).abs() < f32::EPSILON);
        assert!((settings.dock_opacity - 0.84).abs() < f32::EPSILON);
        assert_eq!(settings.cache_limit_mb, DEFAULT_LIMIT_MB);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cache_limit_is_clamped_and_round_trips() {
        let mut settings = UiSettings {
            cache_limit_mb: LIMIT_MAX_MB * 2,
            ..UiSettings::default()
        };
        settings.clamp();
        assert_eq!(settings.cache_limit_mb, LIMIT_MAX_MB);

        // 旧配置里的 512 MB 会被抬到新的下限。
        settings.cache_limit_mb = 512;
        settings.clamp();
        assert_eq!(settings.cache_limit_mb, LIMIT_MIN_MB);

        // 0 仍然表示关闭，不会被夹到下限。
        settings.cache_limit_mb = 0;
        settings.clamp();
        assert_eq!(settings.cache_limit_mb, 0);

        let path = std::env::temp_dir().join("montis-cache-ui-settings.json");
        settings.cache_limit_mb = 4 * 1024;
        settings.save(&path).expect("save");
        assert_eq!(UiSettings::load(&path).cache_limit_mb, 4 * 1024);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disclaimer_is_only_accepted_once() {
        // 旧配置没有这个字段时默认未确认。
        let path = std::env::temp_dir().join("montis-disclaimer-ui-settings.json");
        std::fs::write(&path, r#"{"animation_speed": 1.4}"#).expect("write");
        assert!(!UiSettings::load(&path).disclaimer_accepted);

        let settings = UiSettings {
            disclaimer_accepted: true,
            ..UiSettings::default()
        };
        settings.save(&path).expect("save");
        assert!(UiSettings::load(&path).disclaimer_accepted);
        let _ = std::fs::remove_file(&path);
    }
}

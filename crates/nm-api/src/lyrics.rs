//! LRC 歌词解析。
//!
//! 网易云歌词接口返回的是 `[mm:ss.xx]歌词` 形式的文本，翻译在另一个
//! 字段里；这里把两份文本按时间戳合并成统一的 [`Lyrics`]，播放页只负责
//! 按当前进度取行。

use std::time::Duration;

/// 一行歌词。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LyricLine {
    /// 起始时间（毫秒）。
    pub at_ms: u64,
    pub text: String,
    /// 翻译；没有翻译时为 `None`。
    pub translation: Option<String>,
}

/// 一首歌的歌词，按时间升序排列。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lyrics {
    pub lines: Vec<LyricLine>,
}

impl Lyrics {
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// 当前进度对应的歌词行：最后一个起始时间不晚于 `position` 的行。
    /// 第一行之前返回 `None`（前奏）。
    pub fn active_index(&self, position: Duration) -> Option<usize> {
        if self.lines.is_empty() {
            return None;
        }
        let millis = position.as_millis().min(u64::MAX as u128) as u64;
        self.lines
            .partition_point(|line| line.at_ms <= millis)
            .checked_sub(1)
    }
}

/// 解析原歌词，并把翻译按时间戳合并进来。
pub fn parse_lyrics(lrc: &str, translation: &str) -> Lyrics {
    let mut lines = parse_lrc(lrc);
    if lines.is_empty() {
        return Lyrics { lines };
    }

    let translated = parse_lrc(translation);
    if !translated.is_empty() {
        for line in &mut lines {
            if let Some(matched) = translated.iter().find(|item| item.at_ms == line.at_ms) {
                let text = matched.text.trim();
                if !text.is_empty() && text != line.text {
                    line.translation = Some(text.to_string());
                }
            }
        }
    }

    lines.sort_by_key(|line| line.at_ms);
    Lyrics { lines }
}

/// 解析 LRC 文本。支持一行多个时间戳与 `[mm:ss.xx]` / `[mm:ss.xxx]`
/// 两种小数位数；元数据标签（`[ar:...]`）与空行会被忽略。
pub fn parse_lrc(text: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();

    for raw in text.lines() {
        let mut rest = raw.trim();
        let mut stamps = Vec::new();
        while let Some((at_ms, tail)) = take_timestamp(rest) {
            stamps.push(at_ms);
            rest = tail;
        }
        if stamps.is_empty() {
            continue;
        }

        let content = rest.trim();
        if content.is_empty() {
            continue;
        }

        for at_ms in stamps {
            lines.push(LyricLine {
                at_ms,
                text: content.to_string(),
                translation: None,
            });
        }
    }

    lines.sort_by_key(|line| line.at_ms);
    lines
}

/// 读取行首的 `[mm:ss.xx]`，返回（毫秒，剩余文本）。
fn take_timestamp(line: &str) -> Option<(u64, &str)> {
    let rest = line.strip_prefix('[')?;
    let (tag, tail) = rest.split_once(']')?;
    Some((parse_timestamp(tag)?, tail))
}

fn parse_timestamp(tag: &str) -> Option<u64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let minutes: u64 = minutes.trim().parse().ok()?;
    // 少数播放器用逗号表示毫秒分隔符。
    let seconds = seconds.trim().replace(',', ".");
    let seconds: f64 = seconds.parse().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some(minutes * 60_000 + (seconds * 1_000.0).round() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_lrc() {
        let lines = parse_lrc("[00:12.34]第一句\n[00:15.90]第二句");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].at_ms, 12_340);
        assert_eq!(lines[0].text, "第一句");
        assert_eq!(lines[1].at_ms, 15_900);
    }

    #[test]
    fn supports_multiple_timestamps_and_three_digit_millis() {
        let lines = parse_lrc("[00:01.000][00:03.5]重复");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].at_ms, 1_000);
        assert_eq!(lines[1].at_ms, 3_500);
        assert!(lines.iter().all(|line| line.text == "重复"));
    }

    #[test]
    fn ignores_metadata_and_empty_lines() {
        let lines = parse_lrc("[ti:标题]\n[ar:歌手]\n[00:00.00]\n[00:05.00]正文");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "正文");
    }

    #[test]
    fn merges_translation_by_timestamp() {
        let lyrics = parse_lyrics(
            "[00:01.00]hello\n[00:02.50]world",
            "[00:01.00]你好\n[00:02.50]世界",
        );
        assert_eq!(lyrics.lines[0].translation.as_deref(), Some("你好"));
        assert_eq!(lyrics.lines[1].translation.as_deref(), Some("世界"));
    }

    #[test]
    fn active_line_follows_position() {
        let lyrics = parse_lyrics("[00:01.00]a\n[00:02.00]b", "");
        assert_eq!(lyrics.active_index(Duration::ZERO), None);
        assert_eq!(lyrics.active_index(Duration::from_millis(1_500)), Some(0));
        assert_eq!(lyrics.active_index(Duration::from_millis(2_000)), Some(1));
        assert_eq!(lyrics.active_index(Duration::from_secs(60)), Some(1));
    }
}

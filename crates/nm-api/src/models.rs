//! 音乐内容模型：搜索、歌单与歌曲。
//!
//! 网易云在不同接口里返回的字段大小写不一致（camelCase / snake_case），
//! 这里统一映射成 Rust 侧的名字，并对缺失字段保持宽容。

use serde::{Deserialize, Deserializer, Serialize};

/// 网易云偶尔把布尔字段返回成 `null`，统一当作 `false`。
fn bool_or_false<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<bool>::deserialize(deserializer)?.unwrap_or(false))
}

/// 不可播的播放地址响应里，数字 / 字符串字段经常直接给 `null`，
/// 这些字段对界面无意义，缺失时按空值处理，不能让整条响应解析失败。
fn null_as_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Artist {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Album {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "picUrl")]
    pub pic_url: String,
}

/// 一首歌。搜索与歌单详情共用。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Song {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub ar: Vec<Artist>,
    #[serde(default)]
    pub al: Option<Album>,
    /// 时长（毫秒）。
    #[serde(default)]
    pub dt: u64,
    /// 0 免费 / 1 VIP / 4 购买专辑 / 8 低音质免费。
    #[serde(default)]
    pub fee: i64,
    /// 本地文件路径（仅客户端使用，网络接口不会返回这个字段）。
    ///
    /// 从「音乐文件夹」扫描出来的歌曲靠它播放；也随播放快照与本地收藏
    /// 一起保存，重启后即使扫描还没完成也能直接读文件。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub local_path: String,
}

impl Song {
    pub fn artist_names(&self) -> String {
        if self.ar.is_empty() {
            return "未知歌手".to_string();
        }
        self.ar
            .iter()
            .map(|artist| artist.name.as_str())
            .filter(|name| !name.is_empty())
            .collect::<Vec<_>>()
            .join(" / ")
    }

    pub fn album_name(&self) -> &str {
        self.al
            .as_ref()
            .map(|album| album.name.as_str())
            .unwrap_or("")
    }

    pub fn cover(&self, size: u32) -> String {
        self.al
            .as_ref()
            .map(|album| sized_cover(&album.pic_url, size))
            .unwrap_or_default()
    }

    pub fn duration(&self) -> String {
        format_duration(self.dt)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Creator {
    #[serde(default, rename = "userId")]
    pub user_id: u64,
    #[serde(default)]
    pub nickname: String,
}

/// 歌单摘要，用于侧边栏和卡片。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PlaylistSummary {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "coverImgUrl")]
    pub cover_img_url: String,
    #[serde(default, rename = "trackCount")]
    pub track_count: u64,
    #[serde(default, rename = "playCount")]
    pub play_count: u64,
    #[serde(default)]
    pub creator: Option<Creator>,
    /// true 表示是收藏的歌单，而不是自己创建的。
    #[serde(default, deserialize_with = "bool_or_false")]
    pub subscribed: bool,
    /// 歌单类型：`5` 是账号的「我喜欢的音乐」。
    #[serde(default, rename = "specialType")]
    pub special_type: u32,
}

impl PlaylistSummary {
    pub fn cover(&self, size: u32) -> String {
        sized_cover(&self.cover_img_url, size)
    }
}

/// 歌单里的曲目引用。详情接口只返回前一部分完整歌曲对象，
/// 剩下的要在 `trackIds` 里按顺序找到并另行批量获取。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TrackId {
    #[serde(default)]
    pub id: u64,
}

/// 歌单详情：摘要字段 + 简介 + 歌曲列表。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlaylistDetail {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default, rename = "coverImgUrl")]
    pub cover_img_url: String,
    #[serde(default, rename = "trackCount")]
    pub track_count: u64,
    #[serde(default, rename = "playCount")]
    pub play_count: u64,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub creator: Option<Creator>,
    #[serde(default, deserialize_with = "bool_or_false")]
    pub subscribed: bool,
    #[serde(default)]
    pub tracks: Vec<Song>,
    /// 歌单的完整曲目顺序；可能多于 `tracks`（详情接口只返回前 1000 首）。
    #[serde(default, rename = "trackIds")]
    pub track_ids: Vec<TrackId>,
}

impl PlaylistDetail {
    /// `trackIds` 里有、`tracks` 里还没有的歌曲 id（保持歌单顺序）。
    pub fn missing_track_ids(&self) -> Vec<u64> {
        let known: std::collections::HashSet<u64> =
            self.tracks.iter().map(|song| song.id).collect();
        self.track_ids
            .iter()
            .map(|track| track.id)
            .filter(|id| !known.contains(id))
            .collect()
    }
}

impl PlaylistDetail {
    pub fn cover(&self, size: u32) -> String {
        sized_cover(&self.cover_img_url, size)
    }

    pub fn creator_name(&self) -> &str {
        self.creator
            .as_ref()
            .map(|creator| creator.nickname.as_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("网易云音乐")
    }

    /// 本地收藏一首歌后同步「我喜欢的音乐」页：放到最前面。
    ///
    /// 歌单详情只给出前一部分歌曲，点赞时正在看的列表就是它的可见部分，
    /// 因此这里同时维护 `tracks` 与 `track_ids`，避免取消喜欢后又被补齐任务拉回来。
    pub fn like_track(&mut self, song: Song) {
        if self.tracks.iter().any(|item| item.id == song.id) {
            return;
        }
        let id = song.id;
        self.tracks.insert(0, song);
        if !self.track_ids.iter().any(|track| track.id == id) {
            self.track_ids.insert(0, TrackId { id });
        }
        self.track_count = self
            .track_count
            .saturating_add(1)
            .max(self.tracks.len() as u64);
    }

    /// 取消喜欢后同步「我喜欢的音乐」页：从歌曲与曲目顺序里一起移除。
    pub fn unlike_track(&mut self, id: u64) {
        let before = self.tracks.len();
        self.tracks.retain(|song| song.id != id);
        self.track_ids.retain(|track| track.id != id);
        if self.tracks.len() < before {
            self.track_count = self.track_count.saturating_sub(1);
        }
    }

    pub fn description_or_default(&self) -> &str {
        self.description
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .unwrap_or("这个歌单还没有简介。")
    }
}

/// 一页歌曲搜索结果：`total` 是接口给出的全部命中数，用来判断还能不能翻页。
#[derive(Debug, Clone, Default)]
pub struct SearchPage {
    pub songs: Vec<Song>,
    pub total: u64,
}

/// 歌手资料：简介、头像与作品数量。
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ArtistDetail {
    #[serde(default, deserialize_with = "null_as_default")]
    pub id: u64,
    #[serde(default, deserialize_with = "null_as_default")]
    pub name: String,
    /// 头像。不同接口分别用 `picUrl` / `avatar` / `cover`，统一到这一字段。
    #[serde(default, rename = "picUrl", deserialize_with = "null_as_default")]
    pub pic_url: String,
    #[serde(default, rename = "avatar", deserialize_with = "null_as_default")]
    pub avatar: String,
    #[serde(default, rename = "cover", deserialize_with = "null_as_default")]
    pub cover: String,
    #[serde(default, rename = "briefDesc", deserialize_with = "null_as_default")]
    pub brief_desc: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub alias: Vec<String>,
    #[serde(default, rename = "musicSize", deserialize_with = "null_as_default")]
    pub music_size: u64,
    #[serde(default, rename = "albumSize", deserialize_with = "null_as_default")]
    pub album_size: u64,
    #[serde(default, rename = "mvSize", deserialize_with = "null_as_default")]
    pub mv_size: u64,
}

impl ArtistDetail {
    /// 实际能用的头像地址：`picUrl` 为空时依次退回 `avatar` / `cover`。
    pub fn image(&self) -> &str {
        if !self.pic_url.is_empty() {
            &self.pic_url
        } else if !self.avatar.is_empty() {
            &self.avatar
        } else {
            &self.cover
        }
    }

    /// 别名 / 英文名，展示成「周杰伦 · Jay Chou」中的后半段。
    pub fn subtitle(&self) -> String {
        self.alias
            .iter()
            .filter(|name| !name.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" / ")
    }
}

/// 歌曲音源。`/weapi/song/enhance/player/url/v1` 返回体里的 `data[0]`。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SongUrl {
    #[serde(default, deserialize_with = "null_as_default")]
    pub id: u64,
    /// 无版权 / 需要 VIP 时为 `null`。
    #[serde(default)]
    pub url: Option<String>,
    /// 码率（bps）。
    #[serde(default, deserialize_with = "null_as_default")]
    pub br: u64,
    /// 文件大小（字节）。
    #[serde(default, deserialize_with = "null_as_default")]
    pub size: u64,
    #[serde(default, deserialize_with = "null_as_default")]
    pub level: String,
    /// `mp3` / `flac` / `aac` …
    #[serde(default, rename = "type", deserialize_with = "null_as_default")]
    pub kind: String,
    #[serde(default, deserialize_with = "null_as_default")]
    pub fee: i64,
    /// 单曲维度的业务码，200 表示可用。
    #[serde(default, deserialize_with = "null_as_default")]
    pub code: i64,
}

impl SongUrl {
    pub fn available(&self) -> Option<&str> {
        self.url.as_deref().filter(|url| !url.is_empty())
    }

    /// 不能播放时给用户看的理由。
    pub fn blocked_reason(&self) -> String {
        match self.fee {
            1 => "这首歌需要黑胶 VIP，当前账号无法播放。".to_string(),
            4 => "这首歌需要购买专辑后才能播放。".to_string(),
            _ if self.code == 404 => "这首歌暂无可用音源（可能已下架或没有版权）。".to_string(),
            _ => "这个音源暂时无法播放，换一首试试。".to_string(),
        }
    }

    pub fn quality_label(&self) -> String {
        match self.level.as_str() {
            "standard" => "标准".to_string(),
            "higher" => "较高".to_string(),
            "exhigh" => "极高".to_string(),
            "lossless" => "无损".to_string(),
            "hires" => "Hi-Res".to_string(),
            "jyeffect" => "沉浸环绕".to_string(),
            "sky" => "天空音效".to_string(),
            other => {
                if other.is_empty() {
                    String::new()
                } else {
                    other.to_string()
                }
            }
        }
    }
}

/// 给封面 URL 追加尺寸参数。网易云的图片服务支持 `?param=300y300`。
pub fn sized_cover(url: &str, size: u32) -> String {
    if url.is_empty() {
        return String::new();
    }
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}param={size}y{size}")
}

pub fn format_duration(milliseconds: u64) -> String {
    let total = milliseconds / 1000;
    let minutes = total / 60;
    let seconds = total % 60;
    format!("{minutes}:{seconds:02}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_song_from_cloudsearch() {
        let value = json!({
            "id": 1,
            "name": "晴天",
            "ar": [{ "id": 2, "name": "周杰伦" }],
            "al": { "id": 3, "name": "叶惠美", "picUrl": "https://p1.music.126.net/a.jpg" },
            "dt": 269_000,
            "fee": 0
        });

        let song: Song = serde_json::from_value(value).expect("song");
        assert_eq!(song.name, "晴天");
        assert_eq!(song.artist_names(), "周杰伦");
        assert_eq!(song.album_name(), "叶惠美");
        assert_eq!(song.duration(), "4:29");
        assert_eq!(song.cover(96), "https://p1.music.126.net/a.jpg?param=96y96");
    }

    #[test]
    fn parses_playlist_summary() {
        let value = json!({
            "id": 10,
            "name": "私藏",
            "coverImgUrl": "https://p1.music.126.net/b.jpg",
            "trackCount": 32,
            "playCount": 12345,
            "creator": { "userId": 7, "nickname": "云村听歌人" },
            "subscribed": true
        });

        let playlist: PlaylistSummary = serde_json::from_value(value).expect("playlist");
        assert_eq!(playlist.name, "私藏");
        assert_eq!(playlist.track_count, 32);
        assert!(playlist.subscribed);
        assert_eq!(playlist.special_type, 0);
        assert_eq!(playlist.creator.unwrap().nickname, "云村听歌人");

        let liked: PlaylistSummary = serde_json::from_value(json!({
            "id": 11,
            "name": "我喜欢的音乐",
            "specialType": 5
        }))
        .expect("liked playlist");
        assert_eq!(liked.special_type, 5);
    }

    #[test]
    fn liked_playlist_page_tracks_local_changes() {
        let song = |id: u64| Song {
            id,
            name: format!("歌 {id}"),
            ..Song::default()
        };
        let mut detail = PlaylistDetail {
            track_count: 2,
            tracks: vec![song(1), song(2)],
            track_ids: vec![TrackId { id: 1 }, TrackId { id: 2 }],
            ..PlaylistDetail::default()
        };

        detail.like_track(song(3));
        assert_eq!(detail.tracks[0].id, 3);
        assert_eq!(detail.track_ids[0].id, 3);
        assert_eq!(detail.track_count, 3);
        assert_eq!(detail.missing_track_ids(), Vec::<u64>::new());

        detail.unlike_track(3);
        detail.unlike_track(1);
        assert_eq!(detail.tracks.len(), 1);
        assert_eq!(detail.track_ids.len(), 1);
        assert_eq!(detail.track_count, 1);
        assert!(detail.missing_track_ids().is_empty());
    }

    #[test]
    fn missing_fields_are_tolerated() {
        let song: Song = serde_json::from_value(json!({ "id": 9 })).expect("song");
        assert_eq!(song.name, "");
        assert_eq!(song.artist_names(), "未知歌手");
        assert_eq!(song.cover(64), "");

        let playlist: PlaylistDetail =
            serde_json::from_value(json!({ "id": 3, "name": "空" })).expect("playlist");
        assert_eq!(playlist.creator_name(), "网易云音乐");
        assert_eq!(playlist.description_or_default(), "这个歌单还没有简介。");
        assert!(playlist.track_ids.is_empty());
        assert!(playlist.missing_track_ids().is_empty());
    }

    #[test]
    fn track_ids_reveal_the_songs_the_detail_response_omits() {
        let playlist: PlaylistDetail = serde_json::from_value(json!({
            "id": 1,
            "trackCount": 1500,
            "trackIds": [{ "id": 10 }, { "id": 11 }, { "id": 12 }, { "id": 13 }],
            "tracks": [{ "id": 10 }, { "id": 12 }]
        }))
        .expect("playlist");

        assert_eq!(playlist.track_ids.len(), 4);
        // 只列出缺详情的那几首，顺序跟随歌单。
        assert_eq!(playlist.missing_track_ids(), vec![11, 13]);
    }

    #[test]
    fn null_bool_is_false() {
        let playlist: PlaylistDetail = serde_json::from_value(json!({
            "id": 3,
            "name": "空",
            "subscribed": null
        }))
        .expect("playlist");
        assert!(!playlist.subscribed);
    }

    #[test]
    fn parses_song_url() {
        let value = json!({
            "id": 1,
            "url": "http://m8.music.126.net/a.mp3",
            "br": 320000,
            "size": 1234567,
            "level": "exhigh",
            "type": "mp3",
            "fee": 0,
            "code": 200
        });
        let url: SongUrl = serde_json::from_value(value).expect("song url");
        assert_eq!(url.available(), Some("http://m8.music.126.net/a.mp3"));
        assert_eq!(url.quality_label(), "极高");
        assert_eq!(url.br, 320_000);
    }

    #[test]
    fn blocked_song_url_reports_reason() {
        let vip: SongUrl =
            serde_json::from_value(json!({ "id": 1, "url": null, "fee": 1 })).expect("song url");
        assert!(vip.available().is_none());
        assert!(vip.blocked_reason().contains("VIP"));

        let gone: SongUrl =
            serde_json::from_value(json!({ "id": 2, "code": 404 })).expect("song url");
        assert!(gone.blocked_reason().contains("音源"));
    }

    #[test]
    fn blocked_song_url_tolerates_null_fields() {
        // 非 VIP 账号请求 VIP 歌曲时，接口会把大量字段直接给 null；
        // 解析不能因此失败，否则会被当成「无法解析」而不是「自动跳过」。
        let vip: SongUrl = serde_json::from_value(json!({
            "id": 536622304,
            "url": null,
            "br": null,
            "size": null,
            "md5": null,
            "code": 404,
            "expi": 1200,
            "type": null,
            "gain": 0,
            "peak": 0,
            "fee": 1,
            "payed": 0,
            "flag": 4,
            "canExtend": false,
            "freeTrialInfo": null,
            "level": null,
            "encodeType": null
        }))
        .expect("blocked song url with nulls");

        assert!(vip.available().is_none());
        assert_eq!(vip.fee, 1);
        assert_eq!(vip.code, 404);
        assert!(vip.blocked_reason().contains("VIP"));
    }

    #[test]
    fn artist_detail_prefers_pic_url_and_lists_alias() {
        let value = json!({
            "id": 6452,
            "name": "周杰伦",
            "picUrl": "http://a.jpg",
            "avatar": "http://b.jpg",
            "alias": ["Jay Chou", "周董"],
            "briefDesc": "简介",
            "musicSize": 589,
            "albumSize": 47,
            "mvSize": 9
        });
        let artist: ArtistDetail = serde_json::from_value(value).expect("artist detail");
        assert_eq!(artist.image(), "http://a.jpg");
        assert_eq!(artist.subtitle(), "Jay Chou / 周董");
        assert_eq!(artist.music_size, 589);

        let fallback: ArtistDetail = serde_json::from_value(json!({
            "id": 1,
            "avatar": "http://b.jpg",
            "briefDesc": null,
            "alias": null
        }))
        .expect("artist detail with nulls");
        assert_eq!(fallback.image(), "http://b.jpg");
        assert!(fallback.subtitle().is_empty());
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(59_999), "0:59");
        assert_eq!(format_duration(60_000), "1:00");
        assert_eq!(format_duration(3_600_000), "60:00");
    }
}

//! 网易云 Web API 的最小客户端。
//!
//! 会话完全由手工 Cookie jar 管理（不依赖 reqwest 的 cookie store），
//! 这样登录票据可以被 UI 显式读取、保存和清除。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nm_core::{apply_set_cookie, cookie_header, cookie_value, Cookies, Profile};
use reqwest::header::{HeaderMap, CONTENT_TYPE, COOKIE, ORIGIN, REFERER, SET_COOKIE};
use reqwest::StatusCode;
use serde_json::{json, Value};

use crate::lyrics::{parse_lyrics, Lyrics};
use crate::models::{ArtistDetail, PlaylistDetail, PlaylistSummary, SearchPage, Song, SongUrl};
use crate::{crypto, ApiError, Result};

const BASE: &str = "https://music.163.com";

const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 ",
    "(KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36"
);

/// 二维码登录的一次性凭证。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrLogin {
    pub key: String,
    /// 二维码里实际编码的 URL，供 App 生成图形。
    pub url: String,
}

/// 二维码轮询结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrStatus {
    /// 801：等待扫码。
    Waiting,
    /// 802：已扫码，等待手机端确认。
    Scanned,
    /// 800：二维码过期。
    Expired,
    /// 803：授权成功，Cookie 已写入客户端。
    Authorized,
    /// 8821：网易云风控要求额外完成一次行为验证。
    Verify(QrVerify),
    /// 8821 但没有行为验证参数：网易云直接拒绝了扫码登录（账号环境风险）。
    Blocked {
        message: String,
        redirect: Option<String>,
    },
}

/// 8821 响应中携带的行为验证参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrVerify {
    pub verify_id: String,
    /// 可能是数字也可能是字符串，原样回传给接口。
    pub verify_type: Value,
    pub verify_token: String,
    /// 部分版本的 8821 不再携带 `params`。验证接口允许 `params` 为空对象，
    /// 因此这两个字段只做“有则带上”，缺少时仍然继续走验证流程。
    pub event_id: Option<String>,
    pub sign: Option<String>,
}

impl QrVerify {
    /// 拼装 `getqrcode` 需要的 `params` JSON 字符串。
    fn params_json(&self) -> String {
        let mut params = serde_json::Map::new();
        if let Some(event_id) = &self.event_id {
            params.insert("event_id".to_string(), json!(event_id));
        }
        if let Some(sign) = &self.sign {
            params.insert("sign".to_string(), json!(sign));
        }
        Value::Object(params).to_string()
    }
}

/// 账号统计。`/weapi/w/nuser/account/get` 的 profile 不带这些字段，
/// 需要单独请求 `/weapi/v1/user/detail/{uid}`。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UserStats {
    pub level: u32,
    pub follows: u32,
    pub followeds: u32,
    pub playlist_count: u32,
    pub event_count: u32,
}

/// 行为验证二维码。`url` 就是需要让用户用 App 扫描的内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifyQr {
    pub code: String,
    pub url: String,
}

/// 行为验证二维码的扫描状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyStatus {
    /// qrCodeStatus 0 / detailReason 0：二维码已生成，等待扫码。
    Waiting,
    /// qrCodeStatus 10：已扫码。
    Scanned,
    /// qrCodeStatus 20：验证成功。
    Verified,
    /// qrCodeStatus 21：验证二维码失效。
    Expired,
    /// qrCodeStatus 0 / detailReason 303：手机账号与登录账号不一致。
    Mismatch,
    Unknown {
        code: u32,
        reason: u32,
    },
}

#[derive(Clone)]
pub struct NeteaseClient {
    http: reqwest::Client,
    cookies: Arc<Mutex<Cookies>>,
}

impl NeteaseClient {
    pub fn new() -> Self {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            .connect_timeout(Duration::from_secs(10))
            .gzip(true)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            http,
            cookies: Arc::new(Mutex::new(Cookies::new())),
        }
    }

    pub fn set_cookies(&self, cookies: Cookies) {
        if let Ok(mut jar) = self.cookies.lock() {
            *jar = cookies;
        }
    }

    pub fn snapshot_cookies(&self) -> Cookies {
        self.cookies
            .lock()
            .map(|jar| jar.clone())
            .unwrap_or_default()
    }

    pub fn clear_cookies(&self) {
        if let Ok(mut jar) = self.cookies.lock() {
            jar.clear();
        }
    }

    pub fn cookie_header(&self) -> String {
        self.cookies
            .lock()
            .map(|jar| cookie_header(&jar))
            .unwrap_or_default()
    }

    pub fn has_login_cookie(&self) -> bool {
        self.cookies
            .lock()
            .ok()
            .and_then(|jar| cookie_value(&jar, "MUSIC_U").map(|value| !value.is_empty()))
            .unwrap_or(false)
    }

    /// 申请二维码 key。二维码内容为 `https://music.163.com/login?codekey=...`。
    pub async fn qr_key(&self) -> Result<QrLogin> {
        let body = self
            .request("/weapi/login/qrcode/unikey", json!({ "type": 1 }))
            .await?;

        let key = body
            .get("unikey")
            .and_then(Value::as_str)
            .or_else(|| {
                body.get("data")
                    .and_then(|data| data.get("unikey"))
                    .and_then(Value::as_str)
            })
            .ok_or_else(|| ApiError::Api {
                code: code_of(&body),
                message: message_of(&body),
            })?;

        Ok(QrLogin {
            key: key.to_string(),
            url: format!("https://music.163.com/login?codekey={key}"),
        })
    }

    /// 轮询扫码状态。803 时服务端会通过 `Set-Cookie` 下发 `MUSIC_U`。
    pub async fn qr_check(&self, key: &str) -> Result<QrStatus> {
        let body = self
            .request(
                "/weapi/login/qrcode/client/login",
                json!({ "key": key, "type": 1 }),
            )
            .await?;

        match code_of(&body) {
            800 => Ok(QrStatus::Expired),
            801 => Ok(QrStatus::Waiting),
            802 => Ok(QrStatus::Scanned),
            803 => Ok(QrStatus::Authorized),
            8821 => {
                // 风控返回的字段名在不同版本里并不稳定，留一份原始响应便于排查。
                dump_debug_body(&body);
                Ok(interpret_8821(&body))
            }
            code => Err(ApiError::Api {
                code,
                message: message_of(&body),
            }),
        }
    }

    /// 8821 之后调用：生成用于完成行为验证的二维码。
    pub async fn verify_qr(&self, challenge: &QrVerify) -> Result<VerifyQr> {
        let params = challenge.params_json();

        let body = self
            .request(
                "/api/frontrisk/verify/getqrcode",
                json!({
                    "verifyConfigId": challenge.verify_id,
                    "verifyType": challenge.verify_type,
                    "token": challenge.verify_token,
                    "params": params,
                    "size": 150,
                }),
            )
            .await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        let root = body.get("data").unwrap_or(&body);
        let qr_code = root
            .get("qrCode")
            .and_then(Value::as_str)
            .or_else(|| body.get("qrCode").and_then(Value::as_str))
            .ok_or_else(|| {
                ApiError::Decode(format!("行为验证响应缺少 qrCode：{}", truncate(&body)))
            })?;

        Ok(VerifyQr {
            code: qr_code.to_string(),
            url: verify_url(qr_code, challenge, &params),
        })
    }

    /// 轮询行为验证二维码的扫描状态。
    pub async fn verify_status(&self, qr_code: &str) -> Result<VerifyStatus> {
        let body = self
            .request(
                "/api/frontrisk/verify/qrcodestatus",
                json!({ "qrCode": qr_code }),
            )
            .await?;

        let code = code_of(&body);
        if code > 0 && code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        let root = body.get("data").unwrap_or(&body);
        let Some(status) = root
            .get("qrCodeStatus")
            .or_else(|| body.get("qrCodeStatus"))
            .and_then(as_i64)
        else {
            return Err(ApiError::Decode(format!(
                "验证状态响应缺少 qrCodeStatus：{}",
                truncate(&body)
            )));
        };
        let reason = root
            .get("detailReason")
            .or_else(|| body.get("detailReason"))
            .and_then(as_i64)
            .unwrap_or(0) as u32;

        Ok(match (status as u32, reason) {
            (20, _) => VerifyStatus::Verified,
            (21, _) => VerifyStatus::Expired,
            (10, _) => VerifyStatus::Scanned,
            (0, 0) => VerifyStatus::Waiting,
            (0, 303) => VerifyStatus::Mismatch,
            (code, reason) => VerifyStatus::Unknown { code, reason },
        })
    }

    /// 校验当前 Cookie 并拉取资料。
    ///
    /// 返回 `Ok(None)` 表示“请求成功但没有登录态”，而不是网络错误。
    pub async fn account(&self) -> Result<Option<Profile>> {
        if !self.has_login_cookie() {
            return Ok(None);
        }

        let root = self
            .request("/weapi/w/nuser/account/get", json!({}))
            .await?;

        // 新版接口把结果包在 `data` 里，旧版直接放在顶层，这里两种都兼容。
        let body = root.get("data").unwrap_or(&root);
        let code = body
            .get("code")
            .or_else(|| root.get("code"))
            .and_then(Value::as_i64)
            .unwrap_or(200);

        if code != 200 {
            write_debug_json(&root);
            return Ok(None);
        }

        let Some(profile) = body.get("profile").filter(|value| !value.is_null()) else {
            // 登录态校验失败：把原始响应留给 `MELO_DEBUG_JSON`，方便排查
            // “同一 Cookie 在浏览器 / 其他客户端能用、这里却不行”的情况。
            write_debug_json(&root);
            return Ok(None);
        };

        let profile: Profile = serde_json::from_value(profile.clone())
            .map_err(|error| ApiError::Decode(format!("用户资料格式异常：{error}")))?;

        if profile.user_id == 0 {
            write_debug_json(&root);
            return Ok(None);
        }

        Ok(Some(profile))
    }

    /// 搜索歌曲。`type = 1` 表示单曲，返回歌曲与全部命中数。
    pub async fn search_songs(&self, keyword: &str, limit: u32, offset: u32) -> Result<SearchPage> {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            return Ok(SearchPage::default());
        }

        let body = self
            .request(
                "/weapi/cloudsearch/get/web",
                json!({
                    "s": keyword,
                    "type": 1,
                    "limit": limit,
                    "offset": offset,
                    "total": true,
                }),
            )
            .await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        parse_search_page(&body)
    }

    /// 每日推荐歌曲：按账号口味生成，每天更新一次。
    pub async fn daily_songs(&self) -> Result<Vec<Song>> {
        let body = self
            .request("/weapi/v1/discovery/recommend/songs", json!({}))
            .await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        let songs = parse_daily_songs(&body)?;
        self.complete_songs(songs).await
    }

    /// 私人 FM：按账号口味持续推荐。
    ///
    /// 接口每次只返回 3 首且会忽略 `limit`，所以这里多要几轮再合并去重。
    /// 返回的歌曲大多是旧版结构，展示前还需要补一次歌曲详情。
    pub async fn personal_fm(&self, limit: u32) -> Result<Vec<Song>> {
        let want = limit.max(1) as usize;
        let mut songs: Vec<Song> = Vec::new();
        let mut seen = HashSet::new();

        // 想要 12 首就得请求 4 轮：上限也设成 4，避免接口异常时一直拉。
        for _ in 0..4 {
            let batch = match self.fm_batch().await {
                Ok(batch) => batch,
                // 已经拿到一些就用这些；首轮就失败才把错误抛给界面。
                Err(error) if songs.is_empty() => return Err(error),
                Err(_) => break,
            };
            for song in batch {
                if song.id != 0 && seen.insert(song.id) {
                    songs.push(song);
                }
            }
            if songs.len() >= want {
                break;
            }
        }

        self.complete_songs(songs).await
    }

    /// 私人 FM 的单次请求（接口固定给 3 首）。
    async fn fm_batch(&self) -> Result<Vec<Song>> {
        let body = self.request("/weapi/v1/radio/get", json!({})).await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        parse_fm_songs(&body)
    }

    /// 把推荐接口返回的旧版歌曲结构补齐成完整的 `Song`。
    ///
    /// 旧版只有 id + 歌名（歌手放在 `artists`、时长放在 `duration`），
    /// 直接反序列化会丢掉歌手、专辑、封面与时长。新接口已经完整时不再多发请求。
    async fn complete_songs(&self, songs: Vec<Song>) -> Result<Vec<Song>> {
        if songs.is_empty() {
            return Ok(songs);
        }

        let complete = songs.iter().all(|song| !song.ar.is_empty() && song.dt > 0);
        if complete {
            return Ok(songs);
        }

        let ids: Vec<u64> = songs
            .iter()
            .map(|song| song.id)
            .filter(|id| *id != 0)
            .collect();
        if ids.is_empty() {
            return Ok(songs);
        }

        let detailed = self.songs_by_ids(&ids).await?;
        if detailed.is_empty() {
            return Ok(songs);
        }
        Ok(detailed)
    }

    /// 歌手资料：优先用新版 `head/info/get`，拿不到时退回旧的 `v1/artist/{id}`。
    /// 简介为空时再补一次 `artist/introduction`（某些歌手只有那里有简介）。
    pub async fn artist_detail(&self, id: u64) -> Result<ArtistDetail> {
        let mut detail = self.artist_head(id).await?;
        if detail.brief_desc.trim().is_empty() {
            if let Ok(intro) = self.artist_intro(id).await {
                detail.brief_desc = intro;
            }
        }
        if detail.id == 0 {
            detail.id = id;
        }
        Ok(detail)
    }

    async fn artist_head(&self, id: u64) -> Result<ArtistDetail> {
        let body = self
            .request("/weapi/artist/head/info/get", json!({ "id": id }))
            .await?;
        let code = code_of(&body);
        if code == 200 {
            if let Some(artist) = body
                .pointer("/data/artist")
                .or_else(|| body.get("artist"))
                .cloned()
            {
                return serde_json::from_value(artist)
                    .map_err(|error| ApiError::Decode(format!("歌手详情格式异常：{error}")));
            }
        }

        // 旧接口：`artist` + `hotSongs`，只拿资料部分。
        let body = self
            .request(&format!("/weapi/v1/artist/{id}"), json!({}))
            .await?;
        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }
        let artist = body
            .get("artist")
            .cloned()
            .ok_or_else(|| ApiError::Decode("歌手详情缺少 artist 字段".to_string()))?;
        serde_json::from_value(artist)
            .map_err(|error| ApiError::Decode(format!("歌手详情格式异常：{error}")))
    }

    async fn artist_intro(&self, id: u64) -> Result<String> {
        let body = self
            .request("/weapi/artist/introduction", json!({ "id": id }))
            .await?;
        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }
        Ok(parse_artist_intro(&body))
    }

    /// 歌手的热门歌曲。返回旧版歌曲结构，统一用 `complete_songs` 补齐。
    pub async fn artist_songs(&self, id: u64, limit: u32, offset: u32) -> Result<Vec<Song>> {
        let body = self
            .request(
                "/weapi/v1/artist/songs",
                json!({
                    "id": id,
                    "private_cloud": "true",
                    "work_type": 1,
                    "order": "hot",
                    "offset": offset,
                    "limit": limit,
                }),
            )
            .await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        let songs = body.get("songs").cloned().unwrap_or_else(|| json!([]));
        let songs: Vec<Song> = serde_json::from_value(songs)
            .map_err(|error| ApiError::Decode(format!("歌手歌曲格式异常：{error}")))?;
        self.complete_songs(songs).await
    }

    /// 歌单详情。`n` 一次取足够多，避免详情页只显示前几首。
    pub async fn playlist_detail(&self, id: u64) -> Result<PlaylistDetail> {
        let body = self
            .request(
                "/weapi/v3/playlist/detail",
                json!({ "id": id, "n": 1000, "s": 8 }),
            )
            .await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        let playlist = body
            .get("playlist")
            .cloned()
            .ok_or_else(|| ApiError::Decode("歌单详情缺少 playlist 字段".to_string()))?;
        serde_json::from_value(playlist)
            .map_err(|error| ApiError::Decode(format!("歌单详情格式异常：{error}")))
    }

    /// 按 id 批量取歌曲详情。歌单超过详情接口返回上限（约 1000 首）时，
    /// 用 `trackIds` 把剩下的补齐。一次别传太多：几百个 id 就够。
    pub async fn songs_by_ids(&self, ids: &[u64]) -> Result<Vec<Song>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        // 接口要的是两个 JSON 字符串，而不是数组。
        let c = format!(
            "[{}]",
            ids.iter()
                .map(|id| format!("{{\"id\":{id}}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let ids_param = format!(
            "[{}]",
            ids.iter().map(u64::to_string).collect::<Vec<_>>().join(",")
        );

        let body = self
            .request("/weapi/v3/song/detail", json!({ "c": c, "ids": ids_param }))
            .await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        let songs = body.get("songs").cloned().unwrap_or_else(|| json!([]));
        let songs: Vec<Song> = serde_json::from_value(songs)
            .map_err(|error| ApiError::Decode(format!("歌曲详情格式异常：{error}")))?;

        // 服务端不保证返回顺序，按请求顺序还原。
        let mut by_id: HashMap<u64, Song> = songs.into_iter().map(|song| (song.id, song)).collect();
        Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
    }

    /// 用户创建与收藏的歌单。
    pub async fn user_playlists(&self, user_id: u64) -> Result<Vec<PlaylistSummary>> {
        let body = self
            .request(
                "/weapi/user/playlist",
                json!({ "uid": user_id, "limit": 100, "offset": 0 }),
            )
            .await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        let playlists = body.get("playlist").cloned().unwrap_or_else(|| json!([]));
        serde_json::from_value(playlists)
            .map_err(|error| ApiError::Decode(format!("歌单列表格式异常：{error}")))
    }

    /// 账号统计：等级与关注 / 粉丝 / 歌单 / 动态数。
    pub async fn user_stats(&self, user_id: u64) -> Result<Option<UserStats>> {
        let body = self
            .request(&format!("/weapi/v1/user/detail/{user_id}"), json!({}))
            .await?;

        if code_of(&body) != 200 {
            return Ok(None);
        }

        let profile = body.get("profile").unwrap_or(&body);
        let take = |key: &str| {
            profile
                .get(key)
                .or_else(|| body.get(key))
                .and_then(as_i64)
                .unwrap_or(0)
                .max(0) as u32
        };

        Ok(Some(UserStats {
            level: take("level"),
            follows: take("follows"),
            followeds: take("followeds"),
            playlist_count: take("playlistCount"),
            event_count: take("eventCount"),
        }))
    }

    /// 获取播放地址。`level` 可选 `standard` / `higher` / `exhigh` / `lossless`。
    ///
    /// 无版权或需要 VIP 时接口仍返回 HTTP 200，但 `data[0].url` 为空，
    /// 这里统一转成带业务码的错误。
    pub async fn song_url(&self, id: u64, level: &str) -> Result<SongUrl> {
        let body = self
            .request(
                "/weapi/song/enhance/player/url/v1",
                json!({
                    "ids": format!("[{id}]"),
                    "level": level,
                    "encodeType": "flac",
                }),
            )
            .await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        let data = body
            .pointer("/data/0")
            .cloned()
            .ok_or_else(|| ApiError::Decode("播放地址响应缺少 data".to_string()))?;
        let mut song: SongUrl = serde_json::from_value(data)
            .map_err(|error| ApiError::Decode(format!("播放地址格式异常：{error}")))?;

        if song.id == 0 {
            song.id = id;
        }

        if song.available().is_none() {
            return Err(ApiError::Unavailable(song.blocked_reason()));
        }

        Ok(song)
    }

    /// 获取 LRC 歌词，并把翻译按时间戳合并进来。
    /// 纯音乐 / 没有歌词时返回空列表，而不是报错。
    pub async fn song_lyric(&self, id: u64) -> Result<Lyrics> {
        let body = self
            .request(
                "/weapi/song/lyric?lv=-1&kv=-1&tv=-1",
                json!({ "id": id, "lv": -1, "kv": -1, "tv": -1 }),
            )
            .await?;

        let code = code_of(&body);
        if code != 200 {
            return Err(ApiError::Api {
                code,
                message: message_of(&body),
            });
        }

        Ok(parse_lyrics(
            lyric_text(&body, "lrc"),
            lyric_text(&body, "tlyric"),
        ))
    }

    /// 服务端注销失败不影响本地清理，因此错误会被忽略。
    pub async fn logout(&self) {
        let _ = self.request("/weapi/logout", json!({})).await;
        self.clear_cookies();
    }

    /// 下载头像等静态资源。返回原始字节，交给 UI 层裁剪成圆形。
    pub async fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self
            .http
            .get(url)
            .header(REFERER, BASE)
            .send()
            .await?
            .error_for_status()
            .map_err(|error| match error.status() {
                Some(status) => ApiError::HttpStatus(status.as_u16()),
                None => ApiError::Http(error),
            })?;

        let bytes = response.bytes().await?;
        if bytes.is_empty() {
            return Err(ApiError::Other("资源内容为空".to_string()));
        }
        Ok(bytes.to_vec())
    }

    async fn request(&self, path: &str, payload: Value) -> Result<Value> {
        let params = crypto::weapi(&payload)?;
        let url = format!("{BASE}{path}");

        let mut request = self
            .http
            .post(&url)
            .header(REFERER, BASE)
            .header(ORIGIN, BASE)
            .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
            .form(&[
                ("params", params.params.as_str()),
                ("encSecKey", params.enc_sec_key.as_str()),
            ]);

        let cookie = self.cookie_header();
        if !cookie.is_empty() {
            request = request.header(COOKIE, cookie);
        }

        let response = request.send().await?;
        let status = response.status();
        capture_cookies(self, response.headers());

        let text = response.text().await?;
        let body: Value = serde_json::from_str(&text).map_err(|_| {
            if status == StatusCode::OK {
                ApiError::Decode(format!(
                    "响应不是 JSON：{}",
                    text.chars().take(120).collect::<String>()
                ))
            } else {
                ApiError::HttpStatus(status.as_u16())
            }
        })?;

        if !status.is_success() {
            return Err(ApiError::Api {
                code: code_of(&body),
                message: format!("HTTP {} {}", status.as_u16(), message_of(&body)),
            });
        }

        Ok(body)
    }
}

impl Default for NeteaseClient {
    fn default() -> Self {
        Self::new()
    }
}

fn capture_cookies(client: &NeteaseClient, headers: &HeaderMap) {
    let Ok(mut jar) = client.cookies.lock() else {
        return;
    };
    for value in headers.get_all(SET_COOKIE).iter() {
        if let Ok(raw) = value.to_str() {
            apply_set_cookie(&mut jar, raw);
        }
    }
}

fn code_of(body: &Value) -> i64 {
    body.get("code")
        .or_else(|| body.get("data").and_then(|data| data.get("code")))
        .and_then(as_i64)
        .unwrap_or(-1)
}

fn as_i64(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| {
        value
            .as_str()
            .and_then(|text| text.trim().parse::<i64>().ok())
    })
}

fn message_of(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("msg"))
        .or_else(|| body.get("data").and_then(|data| data.get("message")))
        .and_then(Value::as_str)
        .unwrap_or("未知错误")
        .to_string()
}

/// 歌词接口把文本放在 `lrc.lyric` / `tlyric.lyric`，缺失时按空串处理。
fn lyric_text<'a>(body: &'a Value, key: &str) -> &'a str {
    body.get(key)
        .and_then(|section| section.get("lyric"))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// 搜索结果：歌曲在 `result.songs`，总数在 `result.songCount`。
fn parse_search_page(body: &Value) -> Result<SearchPage> {
    let result = body.get("result").cloned().unwrap_or_else(|| json!({}));
    let songs = result.get("songs").cloned().unwrap_or_else(|| json!([]));
    let songs = serde_json::from_value(songs)
        .map_err(|error| ApiError::Decode(format!("搜索响应格式异常：{error}")))?;
    let total = result.get("songCount").and_then(as_i64).unwrap_or(0).max(0) as u64;
    Ok(SearchPage { songs, total })
}

/// 每日推荐的歌曲列表。新版接口放在 `data.dailySongs`，旧版放在 `recommend`。
fn parse_daily_songs(body: &Value) -> Result<Vec<Song>> {
    let songs = body
        .pointer("/data/dailySongs")
        .or_else(|| body.get("recommend"))
        .or_else(|| body.pointer("/data/recommend"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    serde_json::from_value(songs)
        .map_err(|error| ApiError::Decode(format!("每日推荐格式异常：{error}")))
}

/// 私人 FM 的歌曲列表：接口把歌曲放在 `data` 数组里。
fn parse_fm_songs(body: &Value) -> Result<Vec<Song>> {
    let songs = body.get("data").cloned().unwrap_or_else(|| json!([]));
    serde_json::from_value(songs)
        .map_err(|error| ApiError::Decode(format!("私人FM格式异常：{error}")))
}

/// 歌手简介：优先 `briefDesc`，否则把 `introduction` 的各段拼起来。
fn parse_artist_intro(body: &Value) -> String {
    if let Some(brief) = body.get("briefDesc").and_then(Value::as_str) {
        let brief = brief.trim();
        if !brief.is_empty() {
            return brief.to_string();
        }
    }

    let mut parts = Vec::new();
    if let Some(sections) = body.get("introduction").and_then(Value::as_array) {
        for section in sections {
            if let Some(text) = section.get("tx").and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }
    parts.join("\n\n")
}

/// 区分两种 8821：
/// * 带 `verifyId` / `verifyType` / `verifyToken` → 需要行为验证二维码；
/// * 什么都没有，只有 `message` + `redirectUrl` → 风控直接拒绝本次扫码登录。
fn interpret_8821(body: &Value) -> QrStatus {
    match parse_verify_challenge(body) {
        Some(challenge) => QrStatus::Verify(challenge),
        None => QrStatus::Blocked {
            message: message_of(body),
            redirect: body
                .get("redirectUrl")
                .and_then(Value::as_str)
                .map(ToString::to_string),
        },
    }
}

/// 从 8821 响应中提取行为验证参数。字段位置和命名在不同版本里不固定，
/// 这里递归查找 `verifyId` / `verifyType` / `verifyToken` 与 `event_id` / `sign`，
/// 并兼容 `params` 是对象或 JSON 字符串的形式。
///
/// 只有 `verifyId` / `verifyType` / `verifyToken` 是必需项：`params` 里的
/// `event_id` / `sign` 在新版响应中可能缺失，验证接口本身允许空 `params`。
fn parse_verify_challenge(body: &Value) -> Option<QrVerify> {
    let verify_id = find_named_string(body, &["verifyId", "vid", "verifyConfigId"])?;
    let verify_type = find_named_value(body, &["verifyType", "type"])?;
    let verify_token = find_named_string(body, &["verifyToken", "token"])?;
    let event_id = find_named_string(body, &["event_id", "evid", "eventId"]);
    let sign = find_named_string(body, &["sign"]);

    Some(QrVerify {
        verify_id,
        verify_type,
        verify_token,
        event_id,
        sign,
    })
}

/// 把原始 8821 响应打到终端，并在设置 `MELO_DEBUG_JSON` 时写入文件，
/// 方便对照不同版本的返回结构。
fn dump_debug_body(body: &Value) {
    eprintln!("[melo] 8821 response: {body}");
    write_debug_json(body);
}

/// 仅在设置了 `MELO_DEBUG_JSON` 时写出响应，避免默认情况下把会话数据写到磁盘。
fn write_debug_json(body: &Value) {
    let Ok(path) = std::env::var("MELO_DEBUG_JSON") else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let _ = std::fs::write(path, body.to_string());
}

fn find_named_string(value: &Value, names: &[&str]) -> Option<String> {
    find_named_value(value, names).and_then(|value| match value {
        Value::String(text) => Some(text),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

fn find_named_value(value: &Value, names: &[&str]) -> Option<Value> {
    match value {
        Value::Object(map) => {
            for name in names {
                if let Some(found) = map.get(*name).and_then(scalar_value) {
                    return Some(found);
                }
            }
            for child in map.values() {
                if let Some(found) = find_named_value(child, names) {
                    return Some(found);
                }
            }
            None
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_named_value(child, names)),
        Value::String(text) if text.starts_with('{') || text.starts_with('[') => {
            // `params` 有时是 JSON 字符串。
            serde_json::from_str::<Value>(text)
                .ok()
                .and_then(|parsed| find_named_value(&parsed, names))
        }
        _ => None,
    }
}

fn scalar_value(value: &Value) -> Option<Value> {
    match value {
        Value::String(_) | Value::Number(_) => Some(value.clone()),
        _ => None,
    }
}

fn value_param(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn verify_url(qr_code: &str, challenge: &QrVerify, params: &str) -> String {
    format!(
        "https://st.music.163.com/encrypt-pages?qrCode={qr_code}&verifyToken={}&verifyId={}&verifyType={}&params={params}",
        challenge.verify_token,
        challenge.verify_id,
        value_param(&challenge.verify_type),
    )
}

fn truncate(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 300 {
        format!("{}…", text.chars().take(300).collect::<String>())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_verify_challenge_from_8821() {
        let body = json!({
            "code": 8821,
            "message": "需要行为验证码验证",
            "verifyId": "vid",
            "verifyType": 2,
            "verifyToken": "tok",
            "params": { "event_id": "ev", "sign": "sig" }
        });

        let challenge = parse_verify_challenge(&body).expect("challenge");
        assert_eq!(challenge.verify_id, "vid");
        assert_eq!(challenge.verify_type, json!(2));
        assert_eq!(challenge.verify_token, "tok");
        assert_eq!(challenge.event_id.as_deref(), Some("ev"));
        assert_eq!(challenge.sign.as_deref(), Some("sig"));
    }

    #[test]
    fn parses_nested_verify_challenge_and_params_string() {
        let body = json!({
            "code": 8821,
            "data": {
                "verifyId": "vid",
                "verifyType": "2",
                "verifyToken": "tok",
                "params": "{\"event_id\":\"ev\",\"sign\":\"sig\"}"
            }
        });

        let challenge = parse_verify_challenge(&body).expect("challenge");
        assert_eq!(challenge.verify_id, "vid");
        assert_eq!(challenge.verify_type, json!("2"));
        assert_eq!(challenge.event_id.as_deref(), Some("ev"));
        assert_eq!(challenge.sign.as_deref(), Some("sig"));
        assert_eq!(
            challenge.params_json(),
            "{\"event_id\":\"ev\",\"sign\":\"sig\"}"
        );
    }

    #[test]
    fn parses_alternate_verify_keys() {
        let body = json!({
            "code": 8821,
            "data": {
                "vid": "vid",
                "type": "2",
                "token": "tok",
                "params": { "evid": "ev", "sign": "sig" }
            }
        });

        let challenge = parse_verify_challenge(&body).expect("challenge");
        assert_eq!(challenge.verify_id, "vid");
        assert_eq!(challenge.verify_type, json!("2"));
        assert_eq!(challenge.event_id.as_deref(), Some("ev"));
        assert_eq!(challenge.sign.as_deref(), Some("sig"));
    }

    #[test]
    fn plain_8821_without_verify_fields_is_blocked() {
        let body = json!({
            "code": 8821,
            "message": "请切换其他登录方式或升级新版本再试",
            "redirectUrl": "https://qa-yyy.igame.163.com/anquanhuanjingfengxian"
        });

        assert_eq!(
            interpret_8821(&body),
            QrStatus::Blocked {
                message: "请切换其他登录方式或升级新版本再试".to_string(),
                redirect: Some("https://qa-yyy.igame.163.com/anquanhuanjingfengxian".to_string()),
            }
        );
    }

    #[test]
    fn parses_challenge_without_params() {
        let body = json!({
            "code": 8821,
            "message": "需要行为验证码验证",
            "data": {
                "verifyId": "vid",
                "verifyType": 2,
                "verifyToken": "tok"
            }
        });

        let challenge = parse_verify_challenge(&body).expect("challenge");
        assert_eq!(challenge.verify_id, "vid");
        assert_eq!(challenge.event_id, None);
        assert_eq!(challenge.sign, None);
        assert_eq!(challenge.params_json(), "{}");
    }

    #[test]
    fn parses_search_page_with_total() {
        let body = json!({
            "code": 200,
            "result": {
                "songCount": "123",
                "songs": [ { "id": 1, "name": "甲" }, { "id": 2, "name": "乙" } ]
            }
        });
        let page = parse_search_page(&body).expect("search page");
        assert_eq!(page.total, 123);
        assert_eq!(page.songs.len(), 2);
        assert_eq!(page.songs[1].name, "乙");

        let empty = parse_search_page(&json!({ "code": 200 })).expect("empty");
        assert!(empty.songs.is_empty());
        assert_eq!(empty.total, 0);
    }

    #[test]
    fn parses_daily_songs_from_both_shapes() {
        let modern = json!({
            "code": 200,
            "data": { "dailySongs": [ { "id": 1, "name": "今日" } ] }
        });
        let songs = parse_daily_songs(&modern).expect("modern shape");
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].id, 1);
        assert_eq!(songs[0].name, "今日");

        let legacy = json!({
            "code": 200,
            "recommend": [ { "id": 2, "name": "甲" }, { "id": 3, "name": "乙" } ]
        });
        let songs = parse_daily_songs(&legacy).expect("legacy shape");
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].id, 2);
        assert_eq!(songs[1].name, "乙");

        let empty = parse_daily_songs(&json!({ "code": 200 })).expect("empty");
        assert!(empty.is_empty());
    }

    #[test]
    fn parses_fm_songs_from_data_array() {
        let body = json!({
            "code": 200,
            "data": [ { "id": 11, "name": "甲" }, { "id": 12, "name": "乙" } ]
        });
        let songs = parse_fm_songs(&body).expect("fm shape");
        assert_eq!(songs.len(), 2);
        assert_eq!(songs[0].id, 11);
        assert_eq!(songs[1].name, "乙");

        let empty = parse_fm_songs(&json!({ "code": 200 })).expect("empty");
        assert!(empty.is_empty());
    }

    #[test]
    fn parses_artist_intro_from_both_shapes() {
        let brief = json!({ "briefDesc": "  华语歌手  ", "introduction": [] });
        assert_eq!(parse_artist_intro(&brief), "华语歌手");

        let sections = json!({
            "briefDesc": "",
            "introduction": [
                { "ti": "早年经历", "tx": "第一段" },
                { "ti": "演艺经历", "tx": " 第二段 " },
                { "ti": "空", "tx": "" }
            ]
        });
        assert_eq!(parse_artist_intro(&sections), "第一段\n\n第二段");

        assert_eq!(parse_artist_intro(&json!({})), "");
    }

    #[test]
    fn builds_verify_qr_url() {
        let challenge = QrVerify {
            verify_id: "vid".to_string(),
            verify_type: json!(2),
            verify_token: "tok".to_string(),
            event_id: Some("ev".to_string()),
            sign: Some("sig".to_string()),
        };
        let params = challenge.params_json();
        assert_eq!(
            verify_url("abc", &challenge, &params),
            "https://st.music.163.com/encrypt-pages?qrCode=abc&verifyToken=tok&verifyId=vid&verifyType=2&params={\"event_id\":\"ev\",\"sign\":\"sig\"}"
        );
    }
}

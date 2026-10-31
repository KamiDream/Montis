//! 哔哩哔哩 Web 接口。
//!
//! 只覆盖「把收藏夹当音频听」所需的最小集合：
//! * Cookie 登录校验（`/x/web-interface/nav`）；
//! * 收藏夹列表与收藏夹内容（分页）；
//! * DASH 纯音频流（`/x/player/wbi/playurl`），带 WBI 签名；
//! * 带 `Referer` 的资源下载。
//!
//! 所有请求都直接访问 `api.bilibili.com`，不依赖浏览器或本地服务。

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use nm_core::{apply_set_cookie, cookie_header, cookie_value, Cookies};

use crate::models::{bilibili_song_id, Album, Artist, Song, Source};
use crate::{ApiError, Result};

const BASE: &str = "https://api.bilibili.com";
const REFERER: &str = "https://www.bilibili.com/";
const ORIGIN: &str = "https://www.bilibili.com";
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

const NAV: &str = "/x/web-interface/nav";
const FAV_FOLDERS: &str = "/x/v3/fav/folder/created/list-all";
const FAV_RESOURCES: &str = "/x/v3/fav/resource/list";
const PLAYURL: &str = "/x/player/wbi/playurl";

/// 收藏夹接口每页固定 20 条（服务端上限）。
pub const FOLDER_PAGE_SIZE: u32 = 20;

/// WBI 密钥缓存时间：密钥按天轮换，缓存一小时足够。
const WBI_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BiliUser {
    #[serde(default)]
    pub mid: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub face: String,
}

/// 一个收藏夹（B 站叫「收藏夹」，对应网易云的歌单）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BiliFolder {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cover: String,
    #[serde(default, rename = "media_count")]
    pub media_count: u64,
    /// 0 公开 / 1 私密；默认收藏夹也是 0。
    #[serde(default)]
    pub attr: u32,
}

impl BiliFolder {
    /// 收藏夹封面（按 B 站 CDN 的尺寸语法裁剪）。
    pub fn cover(&self, size: u32) -> String {
        crate::models::sized_cover(&self.cover, size)
    }
}

/// 一页收藏夹内容。
#[derive(Debug, Clone, Default)]
pub struct BiliFolderPage {
    pub title: String,
    pub cover: String,
    pub description: String,
    /// 收藏夹里的视频总数。
    pub total: u64,
    pub songs: Vec<Song>,
    pub has_more: bool,
}

/// 一条可播放的音频流。
#[derive(Debug, Clone, Default)]
pub struct BiliAudio {
    pub url: String,
    /// 音质档位 id（30216 / 30232 / 30280 …），0 表示未知。
    pub id: u64,
    /// 展示用标签。
    pub label: String,
}

#[derive(Clone)]
pub struct BilibiliClient {
    http: reqwest::Client,
    cookies: Arc<Mutex<Cookies>>,
    wbi: Arc<Mutex<Option<WbiKeys>>>,
}

#[derive(Debug, Clone)]
struct WbiKeys {
    mixin: String,
    fetched_at: Instant,
}

impl BilibiliClient {
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
            wbi: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_cookies(&self, cookies: Cookies) {
        if let Ok(mut jar) = self.cookies.lock() {
            *jar = cookies;
        }
        // 换了账号，WBI 密钥与登录态无关，可以继续用；但重新取一次更稳妥。
        if let Ok(mut keys) = self.wbi.lock() {
            *keys = None;
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
            .and_then(|jar| cookie_value(&jar, "SESSDATA").map(|value| !value.is_empty()))
            .unwrap_or(false)
    }

    /// 校验登录态并返回账号资料；Cookie 无效或未登录时返回 `None`。
    pub async fn user(&self) -> Result<Option<BiliUser>> {
        let body = self.send_get(NAV, "").await?;

        if code_of(&body) == -101 {
            return Ok(None);
        }
        ensure_ok(&body)?;

        let data = body.get("data").cloned().unwrap_or(Value::Null);
        self.cache_wbi(&data);

        let logged_in = data
            .get("isLogin")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !logged_in {
            return Ok(None);
        }

        Ok(Some(BiliUser {
            mid: data.get("mid").and_then(Value::as_u64).unwrap_or(0),
            name: data
                .get("uname")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            face: data
                .get("face")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        }))
    }

    /// 列出用户创建（含默认）的收藏夹。
    pub async fn folders(&self, mid: u64) -> Result<Vec<BiliFolder>> {
        if mid == 0 {
            return Err(ApiError::Other(
                "还没有获取到哔哩哔哩账号 UID，请重新连接。".to_string(),
            ));
        }

        let params = vec![("up_mid".to_string(), mid.to_string())];
        let body = self.get_json(FAV_FOLDERS, &params, true).await?;
        ensure_ok(&body)?;
        Ok(parse_folders(&body))
    }

    /// 取一页收藏夹内容。
    pub async fn folder(&self, media_id: u64, page: u32) -> Result<BiliFolderPage> {
        let params = vec![
            ("media_id".to_string(), media_id.to_string()),
            ("pn".to_string(), page.max(1).to_string()),
            ("ps".to_string(), FOLDER_PAGE_SIZE.to_string()),
            ("keyword".to_string(), String::new()),
            ("order".to_string(), "mtime".to_string()),
            ("type".to_string(), "0".to_string()),
            ("tid".to_string(), "0".to_string()),
            ("platform".to_string(), "web".to_string()),
        ];
        let body = self.get_json(FAV_RESOURCES, &params, true).await?;

        let code = code_of(&body);
        if code == -404 {
            return Err(ApiError::Unavailable(
                "这个收藏夹不存在，或者当前账号没有访问权限。".to_string(),
            ));
        }
        ensure_ok(&body)?;

        if body.get("data").map(Value::is_null).unwrap_or(true) {
            return Err(ApiError::Unavailable(
                "这个收藏夹暂时没有内容。".to_string(),
            ));
        }

        Ok(parse_folder_page(&body))
    }

    /// 收藏夹列表接口不返回封面：用第一页第一条视频的封面代替。
    /// 空收藏夹 / 无权限时返回空字符串，界面继续用首字占位。
    pub async fn folder_cover(&self, media_id: u64) -> Result<String> {
        let params = vec![
            ("media_id".to_string(), media_id.to_string()),
            ("pn".to_string(), "1".to_string()),
            ("ps".to_string(), "1".to_string()),
            ("keyword".to_string(), String::new()),
            ("order".to_string(), "mtime".to_string()),
            ("type".to_string(), "0".to_string()),
            ("tid".to_string(), "0".to_string()),
            ("platform".to_string(), "web".to_string()),
        ];
        let body = self.get_json(FAV_RESOURCES, &params, true).await?;
        ensure_ok(&body)?;

        Ok(body
            .pointer("/data/medias/0/cover")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    /// 取视频的纯音频流。优先请求 DASH（`fnval=16`），取码率最高的一条音轨。
    pub async fn audio(&self, bvid: &str, cid: u64) -> Result<BiliAudio> {
        if bvid.is_empty() || cid == 0 {
            return Err(ApiError::Unavailable(
                "这个视频没有可用的音频流。".to_string(),
            ));
        }

        let params = vec![
            ("bvid".to_string(), bvid.to_string()),
            ("cid".to_string(), cid.to_string()),
            ("fnval".to_string(), "16".to_string()),
            ("fnver".to_string(), "0".to_string()),
            ("fourk".to_string(), "1".to_string()),
            ("platform".to_string(), "pc".to_string()),
            ("high_quality".to_string(), "1".to_string()),
        ];

        // 签名请求偶尔会因为风控 / 密钥问题失败，退回不签名再试一次。
        let body = match self.get_json(PLAYURL, &params, true).await {
            Ok(body) if code_of(&body) == 0 => body,
            _ => self.get_json(PLAYURL, &params, false).await?,
        };

        let code = code_of(&body);
        if code != 0 {
            return Err(ApiError::Unavailable(format!(
                "这个视频暂时无法播放：{}",
                message_of(&body)
            )));
        }

        parse_audio(&body).ok_or_else(|| {
            ApiError::Unavailable("这个视频没有返回可用的音频流，可能需要大会员。".to_string())
        })
    }

    /// 下载图片 / 音频。B 站 CDN 需要 `Referer`，但不需要携带登录 Cookie。
    pub async fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let response = self
            .http
            .get(url)
            .header(reqwest::header::REFERER, REFERER)
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

    async fn get_json(
        &self,
        path: &str,
        params: &[(String, String)],
        signed: bool,
    ) -> Result<Value> {
        let query = if signed {
            let keys = self.wbi_keys().await?;
            sign_query(params, &keys.mixin)
        } else {
            encode_params(params)
        };

        self.send_get(path, &query).await
    }

    async fn send_get(&self, path: &str, query: &str) -> Result<Value> {
        let url = if query.is_empty() {
            format!("{BASE}{path}")
        } else {
            format!("{BASE}{path}?{query}")
        };

        let mut request = self
            .http
            .get(&url)
            .header(reqwest::header::REFERER, REFERER)
            .header(reqwest::header::ORIGIN, ORIGIN)
            .header(reqwest::header::ACCEPT, "application/json");

        let cookie = self.cookie_header();
        if !cookie.is_empty() {
            request = request.header(reqwest::header::COOKIE, cookie);
        }

        let response = request.send().await?;
        let status = response.status();

        if let Ok(mut jar) = self.cookies.lock() {
            for value in response.headers().get_all(reqwest::header::SET_COOKIE) {
                if let Ok(text) = value.to_str() {
                    apply_set_cookie(&mut jar, text);
                }
            }
        }

        let text = response.text().await?;
        let body: Value = serde_json::from_str(&text).map_err(|_| {
            if status.is_success() {
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

    /// 取（并缓存）WBI 签名密钥。密钥来自未签名的 `nav` 接口。
    async fn wbi_keys(&self) -> Result<WbiKeys> {
        if let Some(keys) = self.cached_wbi() {
            if keys.fetched_at.elapsed() < WBI_TTL {
                return Ok(keys);
            }
        }

        let body = self.send_get(NAV, "").await?;
        let data = body.get("data").cloned().unwrap_or(Value::Null);
        let keys = wbi_keys_of(&data)
            .ok_or_else(|| ApiError::Other("哔哩哔哩没有返回 WBI 签名所需的密钥。".to_string()))?;

        self.store_wbi(&keys);
        Ok(keys)
    }

    fn cached_wbi(&self) -> Option<WbiKeys> {
        self.wbi.lock().ok().and_then(|keys| keys.as_ref().cloned())
    }

    fn store_wbi(&self, keys: &WbiKeys) {
        if let Ok(mut cached) = self.wbi.lock() {
            *cached = Some(keys.clone());
        }
    }

    fn cache_wbi(&self, data: &Value) {
        if let Some(keys) = wbi_keys_of(data) {
            self.store_wbi(&keys);
        }
    }
}

impl Default for BilibiliClient {
    fn default() -> Self {
        Self::new()
    }
}

fn wbi_keys_of(data: &Value) -> Option<WbiKeys> {
    let img_url = data.pointer("/wbi_img/img_url")?.as_str()?;
    let sub_url = data.pointer("/wbi_img/sub_url")?.as_str()?;
    let img = file_stem(img_url);
    let sub = file_stem(sub_url);
    let mixin = mixin_key(&img, &sub);
    if mixin.len() < 32 {
        return None;
    }
    Some(WbiKeys {
        mixin,
        fetched_at: Instant::now(),
    })
}

fn file_stem(url: &str) -> String {
    url.rsplit('/')
        .next()
        .unwrap_or(url)
        .split('.')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// WBI 文档里的重排表。
const MIXIN_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

fn mixin_key(img: &str, sub: &str) -> String {
    let raw: Vec<char> = format!("{img}{sub}").chars().collect();
    MIXIN_TAB
        .iter()
        .take(32)
        .filter_map(|&index| raw.get(index))
        .collect()
}

fn encode_params(params: &[(String, String)]) -> String {
    let mut pairs: Vec<&(String, String)> = params.iter().collect();
    pairs.sort_by(|left, right| left.0.cmp(&right.0));

    let mut query = String::new();
    for (key, value) in pairs {
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(&percent_encode(key));
        query.push('=');
        query.push_str(&percent_encode(value));
    }
    query
}

/// 按 WBI 规则签名：参数按名称排序、追加 `wts`，再对「查询串 + mixin key」做 MD5。
fn sign_query(params: &[(String, String)], mixin: &str) -> String {
    let wts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    sign_query_at(params, mixin, wts)
}

fn sign_query_at(params: &[(String, String)], mixin: &str, wts: u64) -> String {
    let mut params: Vec<(String, String)> = params.to_vec();
    params.push(("wts".to_string(), wts.to_string()));
    params.sort_by(|left, right| left.0.cmp(&right.0));

    let mut query = String::new();
    for (key, value) in &params {
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(&percent_encode(key));
        query.push('=');
        query.push_str(&percent_encode(value));
    }

    let w_rid = md5_hex(&format!("{query}{mixin}"));
    format!("{query}&w_rid={w_rid}")
}

fn percent_encode(value: &str) -> String {
    // WBI 要求先去掉这几个字符，再做 URL 编码。
    let value = value.replace(['!', '\'', '(', ')', '*'], "");

    let mut encoded = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(*byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

fn md5_hex(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn code_of(body: &Value) -> i64 {
    body.get("code").and_then(Value::as_i64).unwrap_or(-1)
}

fn message_of(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .filter(|message| !message.is_empty())
        .unwrap_or("哔哩哔哩接口返回了错误")
        .to_string()
}

fn ensure_ok(body: &Value) -> Result<()> {
    let code = code_of(body);
    if code == 0 {
        Ok(())
    } else {
        Err(ApiError::Api {
            code,
            message: message_of(body),
        })
    }
}

fn parse_folders(body: &Value) -> Vec<BiliFolder> {
    let list = body
        .pointer("/data/list")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    list.into_iter()
        .filter_map(|item| serde_json::from_value::<BiliFolder>(item).ok())
        .filter(|folder| folder.id != 0)
        .collect()
}

fn parse_folder_page(body: &Value) -> BiliFolderPage {
    let data = body.get("data").cloned().unwrap_or(Value::Null);
    let info = data.get("info").cloned().unwrap_or(Value::Null);

    let title = info
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let cover = info
        .get("cover")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let description = info
        .get("intro")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let total = info.get("media_count").and_then(Value::as_u64).unwrap_or(0);

    let medias = data
        .get("medias")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    let songs = medias
        .iter()
        .filter_map(|media| song_from_media(media, &title))
        .collect();

    let cover = if cover.is_empty() {
        // `info.cover` 经常是空的，退回到第一条视频的封面。
        medias
            .iter()
            .find_map(|media| media.get("cover").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string()
    } else {
        cover
    };

    BiliFolderPage {
        title,
        cover,
        description,
        total,
        songs,
        has_more: data
            .get("has_more")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

/// 视频（收藏项）→ 可播放的 `Song`。没有 `bvid` / `cid` 的条目
/// （例如番剧、已失效视频）跳过，避免列表里出现点了没反应的歌。
fn song_from_media(media: &Value, folder: &str) -> Option<Song> {
    let aid = media.get("id").and_then(Value::as_u64).unwrap_or(0);
    let bvid = media
        .get("bvid")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let cid = media
        .pointer("/ugc/first_cid")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    if aid == 0 || bvid.is_empty() || cid == 0 {
        return None;
    }

    let title = media
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if title.is_empty() || title == "已失效视频" {
        return None;
    }

    let upper = media.get("upper").cloned().unwrap_or(Value::Null);
    let upper_id = upper.get("mid").and_then(Value::as_u64).unwrap_or(0);
    let upper_name = upper
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let cover = media
        .get("cover")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let duration = media.get("duration").and_then(Value::as_u64).unwrap_or(0);

    Some(Song {
        id: bilibili_song_id(aid),
        name: title,
        ar: vec![Artist {
            id: upper_id,
            name: upper_name,
        }],
        // 专辑列借用收藏夹名，列表里比空白更好读。
        al: Some(Album {
            id: 0,
            name: folder.to_string(),
            pic_url: cover,
        }),
        dt: duration.saturating_mul(1000),
        fee: 0,
        source: Source::Bilibili,
        bvid,
        cid,
        local_path: String::new(),
    })
}

fn parse_audio(body: &Value) -> Option<BiliAudio> {
    let data = body.get("data")?;
    let mut candidates: Vec<(u64, u64, String)> = Vec::new();

    if let Some(audios) = data.pointer("/dash/audio").and_then(Value::as_array) {
        for audio in audios {
            push_audio(audio, &mut candidates);
        }
    }
    if let Some(audio) = data.pointer("/dash/flac/audio") {
        push_audio(audio, &mut candidates);
    }
    if let Some(audios) = data.pointer("/dash/dolby/audio").and_then(Value::as_array) {
        for audio in audios {
            push_audio(audio, &mut candidates);
        }
    }

    if let Some(url) = data.pointer("/durl/0/url").and_then(Value::as_str) {
        candidates.push((0, 0, url.to_string()));
    }

    let (bandwidth, id, url) = candidates
        .into_iter()
        .max_by_key(|(bandwidth, _, _)| *bandwidth)?;

    Some(BiliAudio {
        url,
        id,
        label: audio_label(id, bandwidth),
    })
}

fn push_audio(audio: &Value, candidates: &mut Vec<(u64, u64, String)>) {
    let Some(url) = audio
        .get("baseUrl")
        .or_else(|| audio.get("base_url"))
        .and_then(Value::as_str)
    else {
        return;
    };
    if url.is_empty() {
        return;
    }

    let bandwidth = audio.get("bandwidth").and_then(Value::as_u64).unwrap_or(0);
    let id = audio.get("id").and_then(Value::as_u64).unwrap_or(0);

    candidates.push((bandwidth, id, url.to_string()));
}

fn audio_label(id: u64, bandwidth: u64) -> String {
    match id {
        30216 => "64K".to_string(),
        30232 => "132K".to_string(),
        30280 => "192K".to_string(),
        30250 => "杜比全景声".to_string(),
        30251 => "Hi-Res 无损".to_string(),
        0 if bandwidth == 0 => "视频音轨".to_string(),
        _ => "B 站音轨".to_string(),
    }
}

/// 解析用户粘贴的 Cookie。兼容：
/// * `SESSDATA=...; bili_jct=...`；
/// * 只复制 `SESSDATA` 的值；
/// * 从开发者工具 Cookies 表格整行 / 整表复制（制表符分列）；
/// * 带 `Cookie:` 前缀与换行。
pub fn parse_cookie_input(input: &str) -> Cookies {
    let mut cookies = Cookies::new();
    let input = strip_cookie_label(input.trim());
    let input = input.trim_matches('"');

    for line in input.split(['\r', '\n']) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line.contains('\t') {
            let mut columns = line.split('\t').map(str::trim);
            let name = columns.next().unwrap_or_default().trim_end_matches(':');
            let value = columns.next().unwrap_or_default();
            if is_cookie_name(name) && !value.is_empty() {
                cookies.insert(name.to_string(), clean_value(value));
            }
            continue;
        }

        for part in line.split(';') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }

            if let Some((name, value)) = part.split_once('=') {
                let name = name.trim();
                if is_cookie_name(name) {
                    cookies.insert(name.to_string(), clean_value(value));
                }
                continue;
            }

            // 没有 `=`：要么是单独复制的 SESSDATA 值，要么是表格行被
            // 输入框吞掉制表符后的 `SESSDATA<值>域名…`。
            let value = part.strip_prefix("SESSDATA").unwrap_or(part);
            let value = clean_value(value);
            // SESSDATA 只由字母数字和 `% * - _ ,` 组成，遇到域名里的点号就截断。
            let value: String = value
                .chars()
                .take_while(|character| {
                    character.is_ascii_alphanumeric()
                        || matches!(character, '%' | '*' | '-' | '_' | ',')
                })
                .collect();
            if !value.is_empty() {
                cookies.insert("SESSDATA".to_string(), value);
            }
        }
    }

    cookies
}

fn strip_cookie_label(input: &str) -> &str {
    for label in ["cookie:", "set-cookie:", "cookies:"] {
        if input
            .get(..label.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(label))
        {
            return input[label.len()..].trim();
        }
    }
    input
}

fn is_cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.')
        })
}

fn clean_value(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn mixin_key_matches_documented_example() {
        let key = mixin_key(
            "7cd084941338484aae1ad9425b84077c",
            "4932caff0ff746eab6f01bf08b70ac45",
        );
        assert_eq!(key, "ea1db124af3c7062474693fa704f4ff8");
    }

    #[test]
    fn sign_query_matches_documented_example() {
        let params = vec![
            ("foo".to_string(), "114".to_string()),
            ("bar".to_string(), "514".to_string()),
            ("zab".to_string(), "1919810".to_string()),
        ];
        let query = sign_query_at(&params, "ea1db124af3c7062474693fa704f4ff8", 1_702_204_169);
        assert_eq!(
            query,
            "bar=514&foo=114&wts=1702204169&zab=1919810&w_rid=8f6f2b5b3d485fe1886cec6a0be8c5d4"
        );
    }

    #[test]
    fn parses_favorite_folders() {
        let body = json!({
            "code": 0,
            "data": {
                "count": 2,
                "list": [
                    {
                        "id": 998877,
                        "fid": 998877,
                        "mid": 42,
                        "title": "默认收藏夹",
                        "cover": "http://i0.hdslb.com/bfs/archive/cover.jpg",
                        "media_count": 128,
                        "attr": 0
                    },
                    {
                        "id": 0,
                        "title": "坏数据"
                    }
                ]
            }
        });

        let folders = parse_folders(&body);
        assert_eq!(folders.len(), 1);
        assert_eq!(folders[0].id, 998877);
        assert_eq!(folders[0].title, "默认收藏夹");
        assert_eq!(folders[0].media_count, 128);
    }

    #[test]
    fn parses_folder_page_into_songs() {
        let body = json!({
            "code": 0,
            "data": {
                "info": {
                    "title": "深夜电台",
                    "cover": "http://i0.hdslb.com/bfs/archive/folder.jpg",
                    "intro": "睡前听",
                    "media_count": 42
                },
                "medias": [
                    {
                        "id": 170001,
                        "bvid": "BV1xx411c7mD",
                        "title": "海阔天空",
                        "cover": "http://i0.hdslb.com/bfs/archive/a.jpg",
                        "duration": 326,
                        "upper": { "mid": 77, "name": "某 UP 主" },
                        "ugc": { "first_cid": 123456 }
                    },
                    { "id": 170002, "bvid": "", "title": "已失效视频" }
                ],
                "has_more": true
            }
        });

        let page = parse_folder_page(&body);
        assert_eq!(page.title, "深夜电台");
        assert_eq!(page.total, 42);
        assert!(page.has_more);
        assert_eq!(page.songs.len(), 1);

        let song = &page.songs[0];
        assert_eq!(song.id, bilibili_song_id(170001));
        assert!(song.is_bilibili());
        assert_eq!(song.name, "海阔天空");
        assert_eq!(song.bvid, "BV1xx411c7mD");
        assert_eq!(song.cid, 123456);
        assert_eq!(song.dt, 326_000);
        assert_eq!(song.artist_names(), "某 UP 主");
        assert_eq!(song.album_name(), "深夜电台");
    }

    #[test]
    fn folder_page_falls_back_to_first_media_cover() {
        let body = json!({
            "code": 0,
            "data": {
                "info": {
                    "title": "默认收藏夹",
                    "cover": "",
                    "media_count": 2
                },
                "medias": [
                    {
                        "id": 1,
                        "bvid": "BV1",
                        "title": "一",
                        "cover": "http://i0.hdslb.com/a.jpg",
                        "ugc": { "first_cid": 9 }
                    },
                    {
                        "id": 2,
                        "bvid": "BV2",
                        "title": "二",
                        "cover": "http://i0.hdslb.com/b.jpg",
                        "ugc": { "first_cid": 10 }
                    }
                ],
                "has_more": false
            }
        });

        let page = parse_folder_page(&body);
        assert_eq!(page.cover, "http://i0.hdslb.com/a.jpg");
    }

    #[test]
    fn picks_highest_bitrate_audio() {
        let body = json!({
            "code": 0,
            "data": {
                "dash": {
                    "audio": [
                        { "id": 30216, "baseUrl": "https://cdn/64.m4s", "bandwidth": 44000 },
                        { "id": 30280, "base_url": "https://cdn/192.m4s", "bandwidth": 204000 }
                    ]
                }
            }
        });

        let audio = parse_audio(&body).expect("audio");
        assert_eq!(audio.url, "https://cdn/192.m4s");
        assert_eq!(audio.id, 30280);
        assert_eq!(audio.label, "192K");
    }

    #[test]
    fn parses_cookie_forms() {
        let full = parse_cookie_input("SESSDATA=abc%2C123; bili_jct=xyz; DedeUserID=42");
        assert_eq!(full.get("SESSDATA").map(String::as_str), Some("abc%2C123"));
        assert_eq!(full.get("bili_jct").map(String::as_str), Some("xyz"));

        let bare = parse_cookie_input("  abc%2C123*99  ");
        assert_eq!(
            bare.get("SESSDATA").map(String::as_str),
            Some("abc%2C123*99")
        );

        let table = parse_cookie_input("SESSDATA\tabc123\t.hdslb.com\t/");
        assert_eq!(table.get("SESSDATA").map(String::as_str), Some("abc123"));

        // text_input 粘贴时会吞掉制表符，整行只剩 `SESSDATA<值>.hdslb.com/…`。
        let mangled = parse_cookie_input("SESSDATAabc123%2C99*7.hdslb.com/");
        assert_eq!(
            mangled.get("SESSDATA").map(String::as_str),
            Some("abc123%2C99*7")
        );

        let empty = parse_cookie_input("   ");
        assert!(empty.is_empty());
    }

    // ---- 需要联网的手工验证 ----
    // 运行：`cargo test -p nm-api --offline --lib bilibili -- --ignored --nocapture`

    #[tokio::test]
    #[ignore = "需要访问 api.bilibili.com"]
    async fn wbi_signed_playurl_succeeds() {
        let client = BilibiliClient::new();
        let params = vec![
            ("bvid".to_string(), "BV1GJ411x7h7".to_string()),
            ("cid".to_string(), "137649199".to_string()),
            ("fnval".to_string(), "16".to_string()),
            ("fnver".to_string(), "0".to_string()),
            ("fourk".to_string(), "1".to_string()),
            ("platform".to_string(), "pc".to_string()),
            ("high_quality".to_string(), "1".to_string()),
        ];
        let body = client
            .get_json(PLAYURL, &params, true)
            .await
            .expect("signed playurl");
        assert_eq!(code_of(&body), 0, "{}", message_of(&body));
        println!("wbi signed playurl ok");
    }

    #[tokio::test]
    #[ignore = "需要访问 api.bilibili.com"]
    async fn public_audio_stream_is_reachable() {
        let client = BilibiliClient::new();
        let audio = client
            .audio("BV1GJ411x7h7", 137_649_199)
            .await
            .expect("audio stream");
        assert!(!audio.url.is_empty());

        let bytes = client.fetch_bytes(&audio.url).await.expect("audio bytes");
        assert!(bytes.len() > 1024, "音频字节太少：{}", bytes.len());
        println!("audio label = {}, bytes = {}", audio.label, bytes.len());
    }

    #[tokio::test]
    #[ignore = "需要访问 api.bilibili.com"]
    async fn anonymous_nav_has_no_user() {
        let client = BilibiliClient::new();
        assert!(client.user().await.expect("nav").is_none());
    }

    #[tokio::test]
    #[ignore = "需要访问 api.bilibili.com"]
    async fn public_folder_cover_is_reachable() {
        let client = BilibiliClient::new();
        // 公开收藏夹（默认收藏夹），不需要登录 Cookie。
        let cover = client.folder_cover(63_692_673).await.expect("folder cover");
        assert!(cover.starts_with("http"), "封面地址异常：{cover}");

        let bytes = client.fetch_bytes(&cover).await.expect("cover bytes");
        assert!(bytes.len() > 512, "封面字节太少：{}", bytes.len());
        println!("folder cover = {cover}, bytes = {}", bytes.len());
    }
}

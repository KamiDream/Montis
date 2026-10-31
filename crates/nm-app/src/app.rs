use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::mouse;
use iced::widget::{image, scrollable};
use iced::window;
use iced::{Font, Pixels, Point, Size, Subscription, Task};

use rand::Rng;
use serde::{Deserialize, Serialize};

use nm_api::{
    Album, ApiError, Artist, ArtistDetail, BiliFolder, BiliFolderPage, BiliUser, BilibiliClient,
    Creator, NeteaseClient, PlaylistDetail, PlaylistSummary, Profile, SearchPage, Song, UserStats,
};
use nm_audio::{Analyzer, Engine, SpectrumProcessor};
use nm_core::{cookie_header, parse_cookie_input, AppPaths, BilibiliSession, Session};

use crate::audio_cache::{self, AudioCache, MB};
use crate::avatar;
use crate::cover_cache;
use crate::download_store::{self, DownloadStore, LocalTrack};
use crate::library_cache::LibrarySnapshot;
use crate::liked_store::LikedStore;
use crate::local_files::{self, FolderScan, LocalFile};
use crate::player_store::{PlayerSnapshot, DEFAULT_VOLUME};
use crate::ring::RingScene;
use crate::theme;
use crate::ui_settings::UiSettings;
use crate::views;
use crate::visualizer::{VisualField, VisualToggle, VisualizerConfig};
use crate::widgets;

pub type Element<'a> = iced::Element<'a, Message>;

/// 项目主页：关于页里点击后交给系统浏览器打开。
pub(crate) const PROJECT_URL: &str = "https://github.com/KamiDream/Montis";

/// 正在播放页与播放条上的频谱条数。
pub(crate) const BAND_COUNT: usize = 72;
/// 随机播放历史最多保留的条数，避免长时播放无限增长。
const MAX_HISTORY: usize = 200;
/// 启动首帧同步读入的封面数：再多的封面交给异步任务，
/// 免得歌单很多时解码把窗口拖住。
const SYNC_COVER_LIMIT: usize = 40;
/// 滚动停止多久后隐藏滚动条。
const SCROLL_IDLE: Duration = Duration::from_millis(600);
/// 滚动条淡入 / 淡出速度（每秒完成比例）：出现快、消失慢。
const SCROLL_FADE_IN: f32 = 1.0 / 0.12;
const SCROLL_FADE_OUT: f32 = 1.0 / 0.36;
/// 拖动进度条时，指针停稳多久后才真正 seek。
/// 鼠标移动会以每帧频率触发 `SeekTo`，若每次都调用 `try_seek`，
/// 音频线程会不断跳转、积压命令，听感像在倒磁带。
const SEEK_COMMIT_DELAY: Duration = Duration::from_millis(160);

/// 悬浮提示（如「已自动跳过」）的总时长与淡入 / 淡出秒数。
const TOAST_LIFE: Duration = Duration::from_millis(2400);
const TOAST_FADE_IN: f32 = 0.16;
const TOAST_FADE_OUT: f32 = 0.38;
/// 连续自动跳过多少首之后停下，避免整张歌单都不可播时一直空跑。
const MAX_AUTO_SKIP: u8 = 5;
/// 搜索结果每页条数（接口单次最多接受 100）。
const SEARCH_PAGE: u32 = 100;
/// 同时进行的下载数：太多会触发接口风控，太少又显得慢。
const MAX_CONCURRENT_DOWNLOADS: usize = 2;

/// 需要记「最近滚动时间」的区域：只有该区域滚过，它自己的滚动条才出现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollArea {
    /// 歌单里的歌曲列表。
    TrackList,
    /// 播放队列面板。
    Queue,
    /// 播放页的可视化设置面板。
    Panel,
    /// 我的歌单页的歌单网格。
    Library,
    /// 每日推荐页的歌曲列表。
    Daily,
    /// 搜索结果列表。
    Search,
    /// 私人 FM 的歌曲列表。
    Fm,
    /// 歌手页的歌曲列表。
    Artist,
}

impl ScrollArea {
    /// 区域总数：滚动条状态数组按它定长。
    const COUNT: usize = 8;

    fn index(self) -> usize {
        match self {
            Self::TrackList => 0,
            Self::Queue => 1,
            Self::Panel => 2,
            Self::Library => 3,
            Self::Daily => 4,
            Self::Search => 5,
            Self::Fm => 6,
            Self::Artist => 7,
        }
    }
}

/// 滚动条可见性：`now` 与最后一次滚动相差不到 [`SCROLL_IDLE`] 才显示。
fn scroll_recent(last: Option<Instant>, now: Instant) -> bool {
    last.map(|at| now.saturating_duration_since(at) < SCROLL_IDLE)
        .unwrap_or(false)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// 启动时用本地 Cookie 恢复会话。
    Restoring,
    Home,
    /// 正在播放 / 频谱可视化页。
    NowPlaying,
}

/// 循环模式：关 / 列表 / 单曲。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RepeatMode {
    /// 顺序播放：播到队尾就停。
    #[default]
    Off,
    /// 列表循环：播完后回到队首。
    All,
    /// 单曲循环：一直重复当前歌曲。
    One,
}

impl RepeatMode {
    /// 点击循环按钮时依次切换。
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Off => Self::All,
            Self::All => Self::One,
            Self::One => Self::Off,
        }
    }

    /// 队列面板上展示的短标签。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Off => "顺序",
            Self::All => "列表循环",
            Self::One => "单曲循环",
        }
    }
}

/// 播放模式：随机开关与循环模式相互独立，可以同时开启。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct PlayMode {
    pub(crate) shuffle: bool,
    pub(crate) repeat: RepeatMode,
}

impl Serialize for PlayMode {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let mut state = serializer.serialize_struct("PlayMode", 2)?;
        state.serialize_field("shuffle", &self.shuffle)?;
        state.serialize_field("repeat", &self.repeat)?;
        state.end()
    }
}

impl<'de> Deserialize<'de> for PlayMode {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Repr {
            /// 新快照：独立的随机与循环字段。
            Split {
                #[serde(default)]
                shuffle: bool,
                #[serde(default)]
                repeat: RepeatMode,
            },
            /// 旧快照里的单一模式字符串。
            Legacy(String),
        }

        Ok(match Repr::deserialize(deserializer)? {
            Repr::Split { shuffle, repeat } => Self { shuffle, repeat },
            Repr::Legacy(value) => match value.as_str() {
                "shuffle" => Self {
                    shuffle: true,
                    repeat: RepeatMode::Off,
                },
                "repeat_all" => Self {
                    shuffle: false,
                    repeat: RepeatMode::All,
                },
                "repeat_one" => Self {
                    shuffle: false,
                    repeat: RepeatMode::One,
                },
                _ => Self::default(),
            },
        })
    }
}

/// 可请求的音质档位，对应网易云 `song/url/v1` 的 `level` 参数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AudioQuality {
    /// 标准音质。
    Standard,
    /// 较高音质。
    Higher,
    /// 极高音质，默认档位。
    #[default]
    #[serde(rename = "exhigh")]
    ExHigh,
    /// 无损。
    Lossless,
    /// Hi-Res 高解析度。
    #[serde(rename = "hires")]
    HiRes,
}

impl AudioQuality {
    pub(crate) const ALL: [Self; 5] = [
        Self::Standard,
        Self::Higher,
        Self::ExHigh,
        Self::Lossless,
        Self::HiRes,
    ];

    /// 接口的 `level` 参数。
    pub(crate) fn value(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Higher => "higher",
            Self::ExHigh => "exhigh",
            Self::Lossless => "lossless",
            Self::HiRes => "hires",
        }
    }

    /// 设置面板里的标签。
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Standard => "标准",
            Self::Higher => "较高",
            Self::ExHigh => "极高",
            Self::Lossless => "无损",
            Self::HiRes => "Hi-Res",
        }
    }

    /// 音质弹窗里的补充说明。
    pub(crate) fn detail(self) -> &'static str {
        match self {
            Self::Standard => "128 kbps MP3",
            Self::Higher => "192 kbps MP3",
            Self::ExHigh => "320 kbps MP3",
            Self::Lossless => "FLAC 无损",
            Self::HiRes => "Hi-Res 高解析度",
        }
    }
}

impl std::fmt::Display for AudioQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// 搜索框与搜索结果。
#[derive(Default)]
pub(crate) struct SearchState {
    pub(crate) input: String,
    /// 最后一次提交的关键词：翻页用它，而不是可能被编辑过的输入框。
    pub(crate) query: String,
    pub(crate) results: Vec<Song>,
    /// 接口给出的全部命中数。
    pub(crate) total: u64,
    /// 已经从接口取回的条数（翻页偏移；包含被去重的重复项）。
    pub(crate) offset: u32,
    pub(crate) busy: bool,
    /// 正在加载下一页。
    pub(crate) more_busy: bool,
    pub(crate) error: Option<String>,
    /// 是否已经提交过搜索。未提交时首页展示歌单库。
    pub(crate) submitted: bool,
}

/// 悬浮提示：文案 + 出现时间，透明度在渲染时按经过时间计算。
pub(crate) struct Toast {
    pub(crate) text: String,
    pub(crate) shown_at: Instant,
}

/// 本地收藏导入 / 导出弹窗的状态。
pub(crate) struct LikedTransfer {
    pub(crate) kind: LikedTransferKind,
    pub(crate) path: String,
    pub(crate) error: Option<String>,
}

impl LikedTransfer {
    pub(crate) fn new(kind: LikedTransferKind, paths: &AppPaths) -> Self {
        Self {
            kind,
            path: paths.liked_transfer_file().display().to_string(),
            error: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LikedTransferKind {
    Import,
    Export,
}

impl LikedTransferKind {
    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Import => "导入本地收藏",
            Self::Export => "导出本地收藏",
        }
    }

    pub(crate) fn action(self) -> &'static str {
        match self {
            Self::Import => "导入",
            Self::Export => "导出",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Self::Import => "从 JSON 文件读取歌曲，并合并到现有收藏；已收藏的歌会跳过，不会重复。",
            Self::Export => "把当前收藏写成 JSON（与本地 liked.json 同格式），同名文件会被覆盖。",
        }
    }
}

/// 用户添加的一个本地音乐文件夹。
///
/// `files` 是上一次扫描的结果：重新扫描期间保留旧列表，页面不会闪空。
pub(crate) struct LocalFolderState {
    pub(crate) path: PathBuf,
    pub(crate) files: Vec<LocalFile>,
    pub(crate) error: Option<String>,
    pub(crate) scanning: bool,
}

/// 添加 / 管理音乐文件夹的弹窗。
pub(crate) struct LocalFolderDialog {
    pub(crate) path: String,
    pub(crate) error: Option<String>,
}

impl LocalFolderDialog {
    fn new() -> Self {
        // 大多数 Linux 用户把音乐放在 ~/Music，直接预填省得手敲。
        let path = match std::env::var("HOME") {
            Ok(home) if PathBuf::from(&home).join("Music").is_dir() => "~/Music".to_string(),
            _ => String::new(),
        };
        Self { path, error: None }
    }
}

/// 每日推荐歌曲。
#[derive(Default)]
pub(crate) struct DailyState {
    pub(crate) songs: Vec<Song>,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
}

/// 私人 FM 歌曲。
#[derive(Default)]
pub(crate) struct FmState {
    pub(crate) songs: Vec<Song>,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
}

/// 歌手页：资料、简介与歌曲列表。
pub(crate) struct ArtistState {
    pub(crate) id: u64,
    pub(crate) detail: Option<ArtistDetail>,
    pub(crate) songs: Vec<Song>,
    /// 接口给出的作品总数，用来判断能不能继续加载。
    pub(crate) total: u64,
    /// 已经从接口取回的条数（翻页偏移）。
    pub(crate) offset: u32,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
    pub(crate) more_busy: bool,
    /// 简介默认折叠成几行，点击展开查看全文。
    pub(crate) bio_expanded: bool,
    /// 是从播放页点进来的：返回时回到播放页，而不是首页。
    pub(crate) from_player: bool,
}

/// 用户歌单库。
#[derive(Default)]
pub(crate) struct LibraryState {
    pub(crate) playlists: Vec<PlaylistSummary>,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
}

/// 正在查看的歌单详情。
pub(crate) struct PlaylistState {
    pub(crate) id: u64,
    pub(crate) detail: Option<PlaylistDetail>,
    pub(crate) busy: bool,
    pub(crate) error: Option<String>,
    /// 详情接口只返回前一部分歌曲，剩下的正在后台补齐。
    pub(crate) filling: bool,
    /// 这是本地「我喜欢的音乐」页：不请求网络，也不显示“补齐”提示。
    pub(crate) local_liked: bool,
    /// 这是本地「本地音乐」页（用户下载的歌曲）：同样是纯本地列表。
    pub(crate) local_downloads: bool,
    /// 这是哔哩哔哩收藏夹：详情由 B 站接口构建，不走网易云的补齐逻辑。
    pub(crate) bilibili: bool,
    /// 哔哩哔哩收藏夹已加载到第几页。
    pub(crate) bili_page: u32,
    /// 哔哩哔哩收藏夹还有下一页。
    pub(crate) bili_has_more: bool,
    /// 正在加载哔哩哔哩收藏夹的下一页。
    pub(crate) bili_busy: bool,
    /// 歌单内搜索关键词（空 = 不过滤）。
    pub(crate) search: String,
}

impl PlaylistState {
    pub(crate) fn bilibili(id: u64) -> Self {
        Self {
            id,
            detail: None,
            busy: true,
            error: None,
            filling: false,
            local_liked: false,
            local_downloads: false,
            bilibili: true,
            bili_page: 0,
            bili_has_more: false,
            bili_busy: false,
            search: String::new(),
        }
    }
}

/// 哔哩哔哩页面：连接表单与收藏夹列表。
#[derive(Default)]
pub(crate) struct BilibiliState {
    pub(crate) cookie_input: String,
    pub(crate) connecting: bool,
    pub(crate) connect_error: Option<String>,
    pub(crate) folders: Vec<BiliFolder>,
    pub(crate) folders_busy: bool,
    pub(crate) folders_error: Option<String>,
    /// 收藏夹网格是否显示被隐藏的收藏夹。
    pub(crate) show_hidden: bool,
}

/// 播放器状态。音频设备与解码在 `nm_audio::Engine` 的专用线程里。
pub(crate) struct PlayerState {
    pub(crate) engine: Engine,
    /// 当前播放上下文（搜索结果或歌单）。
    pub(crate) queue: Vec<Song>,
    pub(crate) index: usize,
    /// 循环 / 随机模式。
    pub(crate) mode: PlayMode,
    /// 随机模式下的播放历史（索引）；「上一首」按它回退，而不是简单减一。
    pub(crate) history: Vec<usize>,
    pub(crate) current: Option<Song>,
    /// 正在获取播放地址 / 下载音频。
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    /// 平滑后的 0~1 频段电平。
    pub(crate) spectrum: Vec<f64>,
    pub(crate) processor: SpectrumProcessor,
    /// 逐帧 FFT 分析器（缓存计划与缓冲区）。
    pub(crate) analyzer: Analyzer,
    /// 复用给音频线程环形缓冲的采样副本（避免每帧分配）。
    pub(crate) samples: Vec<f32>,
    pub(crate) position: Duration,
    pub(crate) duration: Duration,
    /// 刚发起 seek 时的目标位置；音频线程追上后清空。
    pub(crate) seek_pending: Option<(Duration, Instant)>,
    /// 待提交的 seek（拖动期间只记录目标，停稳后再交给音频线程）。
    pub(crate) seek_commit: Option<(Duration, Instant)>,
    pub(crate) volume: f32,
    /// 静音前的音量；`Some` 表示当前已静音。
    pub(crate) muted_volume: Option<f32>,
    /// 用户选择的音质档位，会随快照保存。
    pub(crate) quality: AudioQuality,
    /// 实际拿到的音源标签（请求档位可能被接口降级）。
    pub(crate) loaded_quality: String,
    /// 换音质前处于暂停：音频加载完成后保持暂停。
    pub(crate) pause_after_load: bool,
    /// 播放请求序号，用来丢弃过期的下载结果。
    pub(crate) load_seq: u64,
    /// 恢复上次退出时的进度；音频加载完成后跳到这个位置。
    pub(crate) pending_resume: Option<Duration>,
    /// 当前队列来自私人 FM：播到队尾时自动续一批新歌。
    pub(crate) fm_queue: bool,
    /// 正在为私人 FM 续歌，避免自动下一首反复发起请求。
    pub(crate) fm_fetching: bool,
    /// 连续自动跳过了多少首（VIP / 无版权）：换歌成功后清零。
    pub(crate) auto_skips: u8,
}

impl PlayerState {
    fn new() -> Self {
        Self {
            engine: Engine::new(),
            queue: Vec::new(),
            index: 0,
            mode: PlayMode::default(),
            history: Vec::new(),
            current: None,
            loading: false,
            error: None,
            spectrum: vec![0.0; BAND_COUNT],
            processor: SpectrumProcessor::new(BAND_COUNT, [1.0, 0.95, 0.80]),
            analyzer: Analyzer::new(BAND_COUNT, 44_100, nm_audio::FFT_SIZE),
            samples: Vec::with_capacity(nm_audio::player::RING_SIZE),
            position: Duration::ZERO,
            duration: Duration::ZERO,
            seek_pending: None,
            seek_commit: None,
            volume: DEFAULT_VOLUME,
            muted_volume: None,
            quality: AudioQuality::default(),
            loaded_quality: String::new(),
            pause_after_load: false,
            load_seq: 0,
            pending_resume: None,
            fm_queue: false,
            fm_fetching: false,
            auto_skips: 0,
        }
    }

    /// 用磁盘快照恢复底部播放条：只把队列、模式与进度放回原处，不自动播放。
    fn restore(&mut self, snapshot: PlayerSnapshot) {
        // 音质是全局设置，即使上次退出时队列已清空也要恢复。
        self.quality = snapshot.quality;

        let Some((index, duration, position)) = restored_track(&snapshot) else {
            return;
        };

        self.index = index;
        self.mode = snapshot.mode;
        self.volume = snapshot.volume.clamp(0.0, 1.0);
        self.engine.set_volume(self.volume);
        self.duration = duration;
        self.position = position;
        self.pending_resume = (!position.is_zero()).then_some(position);
        self.current = Some(snapshot.queue[index].clone());
        self.queue = snapshot.queue;
    }

    pub(crate) fn is_playing(&self) -> bool {
        matches!(self.engine.snapshot().state, nm_audio::State::Playing)
    }

    pub(crate) fn is_muted(&self) -> bool {
        self.muted_volume.is_some()
    }

    /// 播放进度 0~1，时长未知时返回 0。
    pub(crate) fn progress(&self) -> f32 {
        if self.duration.is_zero() {
            0.0
        } else {
            (self.position.as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
        }
    }
}

/// 下载完成的封面：列表用的圆角版本，以及当前曲目大封面额外的圆形版本。
#[derive(Debug, Clone)]
pub struct CoverImage {
    pub bytes: Vec<u8>,
    pub circle: Option<Vec<u8>>,
}

/// 一首已经拿到字节的曲目：音频数据与音源实际档位标签（命中缓存时来自缓存索引）。
#[derive(Debug, Clone)]
pub struct LoadedTrack {
    pub bytes: Vec<u8>,
    pub label: String,
}

/// 加载曲目失败的两类情况。
///
/// * `Skipped`：接口正常返回，但这首歌因为 VIP / 购买 / 版权不可播，
///   直接跳下一首就行；
/// * `Failed`：网络等错误，应该把错误展示给用户。
#[derive(Debug, Clone)]
pub enum TrackError {
    Skipped(String),
    Failed(String),
}

#[derive(Debug, Clone)]
pub enum Message {
    WindowReady(Option<window::Id>),
    WindowResized(Size),
    DragWindow,
    Minimize,
    Close,
    Tick,
    /// 鼠标移动：播放页据此判断指针是否停在底部中央的悬停区。
    PointerMoved(Point),
    CookieChanged(String),
    CookieSubmit,
    /// 在首页内容区打开网易云登录面板。
    OpenLogin,
    /// 从剪贴板读取原始 Cookie（绕过输入框对换行 / 制表符的过滤）。
    PasteCookie,
    CookiePasted(Option<String>),
    SessionChecked(Result<Option<Profile>, String>),
    ProfileLoaded(Result<Option<Profile>, String>),
    CookieChecked(Result<Option<Profile>, String>),
    StatsLoaded(Result<Option<UserStats>, String>),
    AvatarLoaded(Result<Vec<u8>, String>),
    SearchChanged(String),
    SearchSubmit,
    SearchLoaded(Result<SearchPage, String>),
    /// 加载搜索结果下一页。
    SearchMore,
    SearchMoreLoaded(Result<SearchPage, String>),
    ClearSearch,
    LibraryLoaded(Result<Vec<PlaylistSummary>, String>),
    ReloadLibrary,
    OpenPlaylist(u64),
    PlaylistLoaded(Result<PlaylistDetail, String>),
    /// 详情接口只返回前一部分歌曲：后台用 `trackIds` 补齐后一次性追加。
    PlaylistTracksFilled {
        id: u64,
        result: Result<Vec<Song>, String>,
    },
    /// 歌单歌曲列表滚动偏移（虚拟化渲染 + 按需加载封面）。
    PlaylistScrolled(f32),
    /// 歌单内搜索：输入变化 / 提交 / 清除关键词。
    PlaylistSearchChanged(String),
    PlaylistSearchSubmit,
    PlaylistSearchClear,
    /// 只用来让滚动条重新出现：不关心偏移的区域用它上报滚动。
    Scrolled(ScrollArea),
    ClosePlaylist,
    CoverLoaded {
        url: String,
        /// true 表示这是本地缓存的结果（不占用网络请求名额）。
        cached: bool,
        result: Result<CoverImage, String>,
    },
    PlayPause,
    PlaySearchTrack(usize),
    PlayPlaylistTrack(usize),
    NextTrack,
    PrevTrack,
    SeekTo(f32),
    VolumeScrolled(mouse::ScrollDelta),
    ToggleMute,
    OpenNowPlaying,
    CloseNowPlaying,
    ToggleQueue,
    QueuePlay(usize),
    QueueRemove(usize),
    QueueClear,
    ToggleShuffle,
    /// 随机播放当前歌单：确保随机已开启后从第一首开始。
    ShufflePlaylist,
    CycleRepeat,
    QueueScrolled(f32),
    EnqueueSearchTrack(usize),
    EnqueuePlaylistTrack(usize),
    SetQuality(AudioQuality),
    ToggleQualityMenu,
    /// 打开本地收藏的导入 / 导出弹窗。
    OpenLikedTransfer(LikedTransferKind),
    LikedTransferPath(String),
    CloseLikedTransfer,
    ConfirmLikedTransfer,
    ToggleSettings,
    /// 打开首页的独立设置页（界面动画速度等）。
    OpenSettings,
    /// 打开独立的「关于」页（品牌与版本信息）。
    OpenAbout,
    /// 在系统浏览器中打开项目主页。
    OpenProjectUrl,
    /// 首次启动的使用须知：确认后写入设置并关闭弹窗。
    AcceptDisclaimer,
    /// 使用须知弹窗打开时吞掉背景点击。
    DisclaimerBlocked,
    /// 打开哔哩哔哩收藏夹页。
    OpenBilibili,
    BiliCookieChanged(String),
    /// 从剪贴板读取原始 Cookie，绕过输入框对换行 / 制表符的过滤。
    BiliPasteCookie,
    BiliCookiePasted(Option<String>),
    BiliConnect,
    BiliConnected(Result<BiliUser, String>),
    BiliFoldersLoaded(Result<Vec<BiliFolder>, String>),
    /// 收藏夹列表不返回封面：单独拉到的第一页视频封面。
    BiliFolderCover {
        id: u64,
        cover: String,
    },
    /// 隐藏 / 取消隐藏某个收藏夹（设备级偏好，写入 bilibili.json）。
    ToggleBiliFolderHidden(u64),
    /// 在网格里显示 / 收起被隐藏的收藏夹。
    ToggleShowHiddenBiliFolders,
    ReloadBiliFolders,
    OpenBiliFolder(u64),
    BiliFolderLoaded {
        id: u64,
        page: u32,
        result: Result<BiliFolderPage, String>,
    },
    LoadMoreBiliFolder,
    BiliLogout,
    /// 打开独立的搜索页（歌曲搜索从首页拆出）。
    OpenSearch,
    /// 打开每日推荐页。
    OpenDaily,
    DailyLoaded(Result<Vec<Song>, String>),
    ReloadDaily,
    PlayDailyTrack(usize),
    EnqueueDailyTrack(usize),
    /// 打开私人 FM 页。
    OpenFm,
    FmLoaded(Result<Vec<Song>, String>),
    /// 换一批：重新拉一组 FM 推荐。
    ReloadFm,
    /// 播放 FM 列表里的某一首。
    PlayFmTrack(usize),
    EnqueueFmTrack(usize),
    /// 从第一首开始播放 FM。
    PlayAllFm,
    /// 队列播完后自动续上的下一批 FM 歌曲。
    FmMoreLoaded(Result<Vec<Song>, String>),
    /// 打开歌手页（点击歌曲作者）。
    OpenArtist(u64),
    ArtistLoaded {
        id: u64,
        result: Result<(ArtistDetail, Vec<Song>), String>,
    },
    CloseArtist,
    /// 展开 / 收起歌手简介。
    ToggleArtistBio,
    /// 加载更多歌手歌曲。
    ArtistMore,
    ArtistMoreLoaded {
        id: u64,
        result: Result<Vec<Song>, String>,
    },
    PlayArtistTrack(usize),
    EnqueueArtistTrack(usize),
    /// 点击红心：收藏到本地 / 取消收藏。
    ToggleLike(Song),
    /// 打开本地「我喜欢的音乐」页。
    OpenLiked,
    /// 下载一首歌到本地音乐（已下载时只提示，不重复下载）。
    DownloadSong(Song),
    /// 从本地音乐删除一首（本地音乐页里的垃圾桶按钮）。
    RemoveDownload(u64),
    /// 打开本地音乐页。
    OpenDownloads,
    /// 下载任务完成：成功时返回实际音质标签（文件已由任务写入磁盘）。
    Downloaded {
        song: Song,
        result: Result<String, TrackError>,
    },
    /// 打开「清空本地音乐」确认弹窗。
    AskClearDownloads,
    CancelClearDownloads,
    ClearDownloads,
    /// 打开「本地音乐文件夹」弹窗（添加 / 移除 / 重新扫描）。
    OpenLocalFolders,
    CloseLocalFolders,
    LocalFolderPath(String),
    AddLocalFolder,
    RemoveLocalFolder(usize),
    RescanLocalFolders,
    /// 扫描结果；`seq` 用来丢弃已经过期的扫描（刚又改了文件夹列表）。
    LocalFoldersScanned {
        seq: u64,
        folders: Vec<FolderScan>,
    },
    SetAnimationSpeed(f32),
    SetDockOpacity(f32),
    /// 缓存上限（MB），0 表示关闭缓存。
    SetCacheLimit(u64),
    ClearCache,
    VisualValue(VisualField, f32),
    VisualToggle(VisualToggle),
    VisualPalette(crate::visualizer::Palette),
    ResetVisual,
    TrackLoaded {
        id: u64,
        /// 发起请求时的序号，用于丢弃已过期的下载结果。
        seq: u64,
        result: Result<LoadedTrack, TrackError>,
    },
    ScreenshotReady(window::Screenshot),
    RetrySession,
    BackToLogin,
    Logout,
}

pub struct App {
    pub(crate) paths: AppPaths,
    pub(crate) client: NeteaseClient,
    pub(crate) session: Session,
    pub(crate) screen: Screen,
    /// 页面切换的淡入权重：切换时置 1，随后回落到 0。
    pub(crate) screen_fade: f32,
    /// 首页内容区（歌单首页 ↔ 歌单详情）的淡入权重。
    pub(crate) view_fade: f32,
    pub(crate) cookie_input: String,
    pub(crate) cookie_busy: bool,
    pub(crate) login_error: Option<String>,
    pub(crate) restore_error: Option<String>,
    pub(crate) storage_error: Option<String>,
    pub(crate) profile: Option<Profile>,
    pub(crate) avatar: Option<image::Handle>,
    pub(crate) avatar_busy: bool,
    pub(crate) search: SearchState,
    pub(crate) library: LibraryState,
    pub(crate) playlist: Option<PlaylistState>,
    /// 歌单歌曲列表的纵向滚动偏移（虚拟化只渲染可见行）。
    pub(crate) playlist_scroll: f32,
    pub(crate) player: PlayerState,
    /// 环形频谱的动画状态（旋转、拖尾、粒子）。
    pub(crate) ring: RingScene,
    /// 可视化的可调参数，随改随存。
    pub(crate) visual: VisualizerConfig,
    /// 界面偏好（动画速度等），独立保存在 `settings.json`。
    pub(crate) ui: UiSettings,
    /// 下载过的歌曲缓存在本地，命中时不再请求网络。
    pub(crate) cache: Arc<Mutex<AudioCache>>,
    /// 缓存当前占用（字节），设置页展示用。
    pub(crate) cache_usage: u64,
    /// 界面偏好等待落盘的时刻（滑杆拖动时去抖动）。
    ui_save_at: Option<Instant>,
    /// 首页内容区是否正在显示独立的「设置」页。
    pub(crate) settings_page: bool,
    /// 首页内容区是否展示网云登录面板（哔哩哔哩有自己的连接页）。
    pub(crate) netease_login_page: bool,
    /// 首页内容区是否展示独立的「关于」页。
    pub(crate) about_page: bool,
    /// 首页内容区是否展示独立的「搜索」页。
    pub(crate) search_page: bool,
    /// 首页内容区是否展示独立的「每日推荐」页。
    pub(crate) daily_page: bool,
    /// 每日推荐的歌曲列表。
    pub(crate) daily: DailyState,
    /// 首页内容区是否展示独立的「私人 FM」页。
    pub(crate) fm_page: bool,
    /// 私人 FM 的歌曲列表。
    pub(crate) fm: FmState,
    /// 哔哩哔哩接口客户端（与网易云会话相互独立）。
    pub(crate) bili: BilibiliClient,
    /// 哔哩哔哩登录态：Cookie 与账号资料单独保存。
    pub(crate) bili_session: BilibiliSession,
    /// 首页内容区是否展示哔哩哔哩收藏夹。
    pub(crate) bili_page: bool,
    pub(crate) bili_state: BilibiliState,
    /// 本地红心收藏：歌曲与顺序都存在 `liked.json`。
    pub(crate) liked: LikedStore,
    /// 下载到本地的音乐库；文件在数据目录，不受缓存上限影响。
    pub(crate) downloads: Arc<Mutex<DownloadStore>>,
    /// 本地音乐的歌曲列表（最新下载的在前），渲染直接读这份缓存。
    pub(crate) download_tracks: Vec<LocalTrack>,
    /// 每首歌的（音质标签，文件大小），歌曲行里 O(1) 查询下载状态。
    pub(crate) download_info: HashMap<u64, (String, u64)>,
    /// 本地音乐占用（字节）。
    pub(crate) downloads_usage: u64,
    /// 等待下载的歌曲：限制并发，避免一次点太多把接口打满。
    download_queue: VecDeque<Song>,
    /// 已排队、还未开始的歌曲 id。
    download_waiting: HashSet<u64>,
    /// 正在下载的歌曲 id（确定后从队列移到这份集合）。
    download_inflight: HashSet<u64>,
    /// 下载过程中被删除 / 清空的歌曲：任务结束后丢弃结果。
    download_cancelled: HashSet<u64>,
    /// 「清空本地音乐」确认弹窗。
    pub(crate) confirm_clear_downloads: bool,
    /// 用户添加的音乐文件夹（添加顺序）。
    pub(crate) local_folders: Vec<LocalFolderState>,
    /// 所有文件夹里扫描出来的歌曲（去重后、按文件夹顺序）。
    pub(crate) local_tracks: Vec<LocalFile>,
    /// 本地文件夹歌曲的总大小（字节）。
    pub(crate) local_usage: u64,
    /// 歌曲 id →（格式，大小），歌曲行里显示本地文件信息。
    local_info: HashMap<u64, (String, u64)>,
    /// 歌曲 id → 文件路径：按需读取内嵌封面。
    local_cover_sources: HashMap<u64, PathBuf>,
    /// 正在扫描文件夹。
    pub(crate) local_scan_busy: bool,
    /// 扫描序号：改变文件夹列表后丢弃旧结果。
    local_scan_seq: u64,
    /// 添加 / 管理音乐文件夹的弹窗。
    pub(crate) local_folder_dialog: Option<LocalFolderDialog>,
    /// 当前打开的歌手页；`Some` 时优先展示它。
    pub(crate) artist: Option<ArtistState>,
    /// 播放页右侧的可视化设置浮层。
    pub(crate) show_settings: bool,
    /// 音质选择弹窗。
    pub(crate) show_quality: bool,
    /// 本地收藏导入 / 导出弹窗；`None` 表示关闭。
    pub(crate) liked_transfer: Option<LikedTransfer>,
    pub(crate) show_queue: bool,
    /// 队列面板纵向滚动偏移，用于虚拟化只渲染可见行。
    pub(crate) queue_scroll: f32,
    /// 各区域最后一次滚动的时间：0.6 秒内滚过才画滚动条。
    scroll_touched: [Option<Instant>; ScrollArea::COUNT],
    /// 各区域滚动条的当前不透明度（0 = 隐藏）：出现 / 消失都走淡入淡出。
    scroll_fade: [f32; ScrollArea::COUNT],
    pub(crate) covers: HashMap<String, image::Handle>,
    /// 当前曲目的圆形封面：正在播放页把专辑图裁成圆，需要单独的 Handle。
    pub(crate) circle_covers: HashMap<String, image::Handle>,
    /// 已经查过本地缓存的封面 URL（每个 URL 每次启动只查一次）。
    cover_disk: HashSet<String>,
    /// 本次启动已经刷新过的封面 URL：每个封面每次启动重新下载一次。
    cover_refreshed: HashSet<String>,
    /// 刷新已经成功应用过的 URL：防止慢一帧的缓存结果反向覆盖新图。
    cover_fresh: HashSet<String>,
    pub(crate) window: Option<window::Id>,
    /// 当前窗口尺寸，用于决定歌单网格的列数。
    pub(crate) viewport: Size,
    pub(crate) phase: f32,
    /// 按真实帧间隔累加的秒数，供「不确定进度」等与帧率无关的动画使用。
    pub(crate) clock: f32,
    /// 加载光效的权重：加载中淡入到 1，结束后淡出到 0。
    pub(crate) load_mix: f32,
    /// 控制台滑出 / 滑入的过渡进度：1 = 完全展开，0 = 完全收起。
    pub(crate) dock_mix: f32,
    /// 最近一次鼠标位置（窗口坐标），用于判断指针是否停在底部悬停区。
    pointer: Point,
    /// 是否已经收到过鼠标位置：进入播放页后的第一次移动才让控制台有资格展开，
    /// 保证刚打开页面时一定是收起状态。
    pointer_known: bool,
    /// 仅 debug 构建可用：MELO_SHOW_DOCK=1 时固定展开控制台（截图用）。
    dock_pinned: bool,
    /// 仅 debug 构建可用：用本地假数据预览首页。
    demo: bool,
    screenshot_path: Option<PathBuf>,
    screenshot_at: Option<Instant>,
    last_tick: Instant,
    /// 播放状态的最近一次落盘时间，避免高频写盘。
    last_player_save: Instant,
    /// 队列 / 模式等有变化，等下一次 Tick 统一落盘。
    player_dirty: bool,
    /// 全局悬浮提示（自动跳过等）。
    pub(crate) toast: Option<Toast>,
}

pub fn run() -> iced::Result {
    let (default_font, fonts) = system_cjk_font();

    let size = window_size();

    iced::application("Montis", App::update, App::view)
        .theme(|_| theme::app_theme())
        .settings(iced::Settings {
            id: Some("melody".to_string()),
            fonts,
            default_font,
            default_text_size: Pixels(14.0),
            antialiasing: true,
        })
        .window(window::Settings {
            size,
            min_size: Some(Size::new(980.0, 640.0)),
            position: window::Position::Centered,
            decorations: false,
            resizable: true,
            ..window::Settings::default()
        })
        .subscription(App::subscription)
        .run_with(App::new)
}

/// 窗口默认尺寸；调试时可用 `MELO_WINDOW_SIZE=1600x900` 指定，方便检查响应式布局。
fn window_size() -> Size {
    #[cfg(debug_assertions)]
    if let Ok(value) = std::env::var("MELO_WINDOW_SIZE") {
        if let Some((width, height)) = value.split_once('x') {
            if let (Ok(width), Ok(height)) = (width.parse(), height.parse()) {
                return Size::new(width, height);
            }
        }
    }

    Size::new(1120.0, 740.0)
}

impl App {
    fn new() -> (Self, Task<Message>) {
        let paths = AppPaths::discover();
        let _ = paths.ensure();
        // 封面缓存只做加速：启动时顺手把过多的旧文件清掉。
        cover_cache::prune(&paths.cover_cache_dir(), cover_cache::MAX_FILES);

        let (session, warning) = Session::load(&paths.session_file());
        let client = NeteaseClient::new();

        // 哔哩哔哩登录态单独保存：与网易云会话互不影响。
        let (bili_session, bili_warning) = BilibiliSession::load(&paths.bilibili_file());
        let bili = BilibiliClient::new();
        let bili_logged_in = bili_session.is_logged_in();
        if bili_logged_in {
            bili.set_cookies(bili_session.cookies.clone());
        }

        let restoring = session.is_logged_in();
        if restoring {
            client.set_cookies(session.cookies.clone());
        }
        // 会话文件里还留着上次的资料：没有它时先停在恢复页，有它就直接进首页。
        let cached_profile = session.profile.clone();

        // 开发辅助：在没有真实账号时预览登录后的布局。
        #[cfg(debug_assertions)]
        let demo = std::env::var("MELO_DEMO_USER")
            .map(|value| value == "1")
            .unwrap_or(false);
        #[cfg(not(debug_assertions))]
        let demo = false;

        let mut library = LibraryState::default();
        if demo {
            library.playlists = demo_playlists();
        }

        // 本地已有资料时不必停在恢复页：首页先用快照与封面缓存撑起来，
        // 会话校验（`SessionChecked`）与歌单刷新都在后台继续。
        // 只登录了哔哩哔哩时也直接进首页，默认打开哔哩哔哩分区。
        let bili_only = bili_logged_in && !restoring && !demo;
        let start_home = demo || (restoring && cached_profile.is_some()) || bili_logged_in;
        if start_home && !demo && restoring {
            library.busy = true;
        }

        let visual = VisualizerConfig::load(&paths.visualizer_file());
        let ui = UiSettings::load(&paths.settings_file());

        // 音频缓存：上限来自设置，启动时先按上限清理一次。
        let cache = Arc::new(Mutex::new(AudioCache::open(
            paths.audio_cache_dir(),
            ui.cache_limit_mb * MB,
        )));
        let cache_usage = cache
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .usage();

        // 底部播放条的记忆：队列、当前歌曲、进度、模式与音量。
        let mut player = PlayerState::new();
        player.restore(PlayerSnapshot::load(&paths.player_file()));

        // 本地红心收藏：只读本地文件，不经过云端。
        let liked = LikedStore::load(&paths.liked_file());

        // 本地音乐：用户主动下载、长期保留的歌曲。
        let downloads = Arc::new(Mutex::new(DownloadStore::open(paths.music_dir())));
        let (download_tracks, download_info, downloads_usage) = {
            let store = downloads.lock().unwrap_or_else(|error| error.into_inner());
            let tracks = store.tracks();
            let info = tracks
                .iter()
                .map(|track| (track.song.id, (track.label.clone(), track.size)))
                .collect();
            (tracks, info, store.usage())
        };

        // 本地音乐文件夹：只保存路径，启动时在后台扫描（不阻塞首帧）。
        let folder_paths = local_files::load_folders(&paths.music_folders_file());
        let local_scan_busy = !folder_paths.is_empty();
        let local_folders: Vec<LocalFolderState> = folder_paths
            .iter()
            .map(|path| LocalFolderState {
                path: path.clone(),
                files: Vec::new(),
                error: None,
                scanning: local_scan_busy,
            })
            .collect();
        let local_scan_seq = 1;
        let local_scan_task = if folder_paths.is_empty() {
            Task::none()
        } else {
            Task::perform(local_files::scan_folders(folder_paths), move |folders| {
                Message::LocalFoldersScanned {
                    seq: local_scan_seq,
                    folders,
                }
            })
        };

        #[cfg(debug_assertions)]
        let show_settings = std::env::var("MELO_SHOW_SETTINGS").as_deref() == Ok("1");
        #[cfg(not(debug_assertions))]
        let show_settings = false;

        #[cfg(debug_assertions)]
        let show_queue = std::env::var("MELO_SHOW_QUEUE").as_deref() == Ok("1");
        #[cfg(not(debug_assertions))]
        let show_queue = false;

        #[cfg(debug_assertions)]
        let show_quality = std::env::var("MELO_SHOW_QUALITY").as_deref() == Ok("1");
        #[cfg(not(debug_assertions))]
        let show_quality = false;

        #[cfg(debug_assertions)]
        let liked_transfer = match std::env::var("MELO_SHOW_LIKED_TRANSFER").as_deref() {
            Ok("import") => Some(LikedTransfer::new(LikedTransferKind::Import, &paths)),
            Ok("export") => Some(LikedTransfer::new(LikedTransferKind::Export, &paths)),
            _ => None,
        };
        #[cfg(not(debug_assertions))]
        let liked_transfer = None;

        #[cfg(debug_assertions)]
        let show_about = std::env::var("MELO_SHOW_ABOUT").as_deref() == Ok("1");
        #[cfg(not(debug_assertions))]
        let show_about = false;

        #[cfg(debug_assertions)]
        let show_settings_page = std::env::var("MELO_SHOW_SETTINGS_PAGE").as_deref() == Ok("1");
        #[cfg(not(debug_assertions))]
        let show_settings_page = false;

        let app = Self {
            paths,
            client,
            session,
            // 没有账号时也不再停在登录屏：直接进首页，登录面板在内容区里打开。
            screen: Screen::Home,
            // 启动时也做一次淡入，避免首帧硬切。
            screen_fade: 1.0,
            view_fade: 0.0,
            cookie_input: String::new(),
            cookie_busy: false,
            login_error: None,
            restore_error: None,
            storage_error: warning.or(bili_warning),
            profile: if demo {
                Some(demo_profile())
            } else if start_home {
                cached_profile
            } else {
                None
            },
            avatar: None,
            avatar_busy: false,
            search: SearchState::default(),
            library,
            playlist: None,
            playlist_scroll: 0.0,
            player,
            ring: RingScene::new(),
            visual,
            ui,
            cache,
            cache_usage,
            ui_save_at: None,
            settings_page: show_settings_page,
            netease_login_page: !restoring && !bili_logged_in && !demo,
            about_page: show_about,
            search_page: false,
            daily_page: false,
            daily: DailyState::default(),
            fm_page: false,
            fm: FmState::default(),
            bili,
            bili_session,
            bili_page: bili_only,
            bili_state: BilibiliState::default(),
            liked,
            downloads,
            download_tracks,
            download_info,
            downloads_usage,
            download_queue: VecDeque::new(),
            download_waiting: HashSet::new(),
            download_inflight: HashSet::new(),
            download_cancelled: HashSet::new(),
            confirm_clear_downloads: false,
            local_folders,
            local_tracks: Vec::new(),
            local_usage: 0,
            local_info: HashMap::new(),
            local_cover_sources: HashMap::new(),
            local_scan_busy,
            local_scan_seq,
            local_folder_dialog: None,
            artist: None,
            show_settings,
            show_quality,
            liked_transfer,
            show_queue,
            queue_scroll: 0.0,
            scroll_touched: [None; ScrollArea::COUNT],
            scroll_fade: [0.0; ScrollArea::COUNT],
            covers: HashMap::new(),
            circle_covers: HashMap::new(),
            cover_disk: HashSet::new(),
            cover_refreshed: HashSet::new(),
            cover_fresh: HashSet::new(),
            window: None,
            viewport: window_size(),
            phase: 0.0,
            clock: 0.0,
            load_mix: 0.0,
            dock_mix: 0.0,
            pointer: Point::ORIGIN,
            pointer_known: false,
            dock_pinned: cfg!(debug_assertions)
                && std::env::var("MELO_SHOW_DOCK").as_deref() == Ok("1"),
            demo,
            screenshot_path: std::env::var("MELO_SCREENSHOT")
                .ok()
                .filter(|path| !path.is_empty())
                .map(PathBuf::from),
            screenshot_at: std::env::var("MELO_SCREENSHOT")
                .ok()
                .filter(|path| !path.is_empty())
                .map(|_| {
                    let delay = std::env::var("MELO_SCREENSHOT_DELAY_MS")
                        .ok()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(1500);
                    Instant::now() + Duration::from_millis(delay)
                }),
            last_tick: Instant::now(),
            last_player_save: Instant::now(),
            player_dirty: false,
            toast: None,
        };

        let window_task = iced::window::get_latest().map(Message::WindowReady);
        // 只有 debug 构建的演示页预览会修改 app，release 下不需要 mut。
        #[allow(unused_mut)]
        let mut app = app;

        // 开发辅助：MELO_DEMO_PAGE=search / playlist 直接预览第二步页面。
        #[cfg(debug_assertions)]
        if demo {
            // 先铺好感演示红心，后面打开本地收藏页时才有内容。
            if std::env::var("MELO_LIKED").as_deref() == Ok("1") {
                app.liked.songs = demo_songs();
            }
            match std::env::var("MELO_DEMO_PAGE").as_deref() {
                Ok("search") => {
                    app.search.input = "周杰伦".to_string();
                    app.search.results = demo_songs();
                    app.search.submitted = true;
                    app.search_page = true;
                }
                Ok("daily") => {
                    app.daily_page = true;
                    app.daily.songs = demo_songs();
                    app.daily.busy = false;
                }
                Ok("fm") => {
                    app.fm_page = true;
                    app.fm.songs = demo_songs();
                    app.fm.busy = false;
                }
                Ok("artist") => {
                    let _ = app.open_artist(2001);
                }
                Ok("bilibili") => {
                    // 只预览连接页；没有 Cookie 时展示表单。
                    app.bili_page = true;
                }
                Ok("playlist") => {
                    app.playlist = Some(PlaylistState {
                        id: 1000,
                        detail: Some(demo_detail(1000)),
                        busy: false,
                        error: None,
                        filling: false,
                        local_liked: false,
                        local_downloads: false,
                        bilibili: false,
                        bili_page: 0,
                        bili_has_more: false,
                        bili_busy: false,
                        search: String::new(),
                    });
                }
                Ok("liked") => {
                    let _ = app.open_liked();
                }
                Ok("downloads") => {
                    // 演示页没有真实文件：先打开空页，再塞入内存里的假下载记录。
                    let _ = app.open_downloads();
                    app.download_tracks = demo_songs()
                        .into_iter()
                        .enumerate()
                        .map(|(index, song)| LocalTrack {
                            song,
                            label: ["无损", "极高", "标准"][index % 3].to_string(),
                            size: 9_000_000 + index as u64 * 1_700_000,
                        })
                        .collect();
                    app.download_info = app
                        .download_tracks
                        .iter()
                        .map(|track| (track.song.id, (track.label.clone(), track.size)))
                        .collect();
                    app.downloads_usage = app.download_tracks.iter().map(|track| track.size).sum();
                    app.sync_downloads_page();
                }
                _ => {}
            }
            // MELO_SHOW_PLAYER=1 时直接进入正在播放页，方便截图检查可视化。
            if std::env::var("MELO_SHOW_PLAYER").as_deref() == Ok("1") {
                app.go(Screen::NowPlaying);
                let _ = app.play_from(demo_songs(), 0);
            }
        }

        // 开发辅助：真实账号下直接打开本地音乐页（截图 / 手动检查用）。
        #[cfg(debug_assertions)]
        let open_downloads_task = if std::env::var("MELO_OPEN_DOWNLOADS").as_deref() == Ok("1") {
            app.open_downloads()
        } else {
            Task::none()
        };
        #[cfg(not(debug_assertions))]
        let open_downloads_task = Task::none();
        #[cfg(debug_assertions)]
        if std::env::var("MELO_SHOW_CLEAR_DOWNLOADS").as_deref() == Ok("1") {
            app.confirm_clear_downloads = true;
        }
        #[cfg(debug_assertions)]
        if std::env::var("MELO_SHOW_LOCAL_FOLDERS").as_deref() == Ok("1") {
            app.local_folder_dialog = Some(LocalFolderDialog::new());
        }

        let boot = if demo {
            Task::none()
        } else if restoring {
            Task::perform(load_account(app.client.clone()), Message::SessionChecked)
        } else {
            Task::none()
        };

        // 首页直接用上次的歌单快照与磁盘封面缓存渲染：首帧就是封面，
        // 而不是一堆文字占位；网络回来后由 `LibraryLoaded` 原地替换。
        if start_home && !demo {
            app.restore_library_cache();
            app.prime_cached_avatar();
        }

        // 恢复出来的当前歌曲需要封面，播放条和正在播放页都会用到。
        let mut tasks = vec![window_task, boot, open_downloads_task, local_scan_task];

        // 只登录了哔哩哔哩：启动就直接拉收藏夹与头像，不用等用户点菜单。
        if bili_only && !demo {
            let face = nm_api::sized_cover(&app.bili_session.face, 600);
            if !face.is_empty() {
                tasks.push(app.ensure_covers(vec![face]));
            }
            tasks.push(app.load_bili_folders());
        }
        if let Some(url) = app.player.current.as_ref().map(|song| song.cover(600)) {
            app.prime_cached_covers(vec![url.clone()]);
            tasks.push(app.ensure_covers(vec![url]));
        }

        (app, Task::batch(tasks))
    }

    fn subscription(&self) -> Subscription<Message> {
        // 加载与页面切换动画需要流畅：淡出期间都按 60fps 推进；
        // 其余情况播放页保持满帧，其他页面降到 20fps。
        let interval = if self.screen == Screen::NowPlaying {
            // 播放页始终满帧：暂停时频谱、粒子与底部控制台的过渡也靠它推进。
            if self.visual.high_fps {
                8
            } else {
                16
            }
        } else if self.player.loading
            || self.load_mix > 0.0
            || self.screen_fade > 0.0
            || self.view_fade > 0.0
            || self.scroll_animating()
            // 提示的淡入淡出很短：它存在期间抬到 60fps，否则 20fps 会一跳一跳。
            || self.toast.is_some()
            || self.settings_page
            || self.netease_login_page
            || self.about_page
        {
            16
        } else {
            50
        };
        Subscription::batch([
            iced::time::every(Duration::from_millis(interval)).map(|_| Message::Tick),
            iced::window::resize_events().map(|(_, size)| Message::WindowResized(size)),
            // 只有播放页需要知道指针活动：用它判断控制台什么时候该收起 / 叫回来。
            if self.screen == Screen::NowPlaying {
                iced::event::listen_with(dock_pointer_event)
            } else {
                Subscription::none()
            },
        ])
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::WindowReady(id) => {
                self.window = id;
                match id {
                    Some(id) => window::get_size(id).map(Message::WindowResized),
                    None => Task::none(),
                }
            }
            Message::WindowResized(size) => {
                self.viewport = size;
                Task::none()
            }
            Message::DragWindow => match self.window {
                Some(id) => window::drag(id),
                None => Task::none(),
            },
            Message::Minimize => match self.window {
                Some(id) => window::minimize(id, true),
                None => Task::none(),
            },
            Message::Close => {
                self.persist_player();
                self.flush_ui_settings();
                match self.window {
                    Some(id) => window::close(id),
                    None => iced::exit(),
                }
            }
            Message::Tick => {
                let now = Instant::now();
                let dt = (now - self.last_tick).as_secs_f32().clamp(0.001, 0.1);
                self.last_tick = now;
                // 悬浮提示到时自动消失。
                if self
                    .toast
                    .as_ref()
                    .is_some_and(|toast| toast.shown_at.elapsed() >= TOAST_LIFE)
                {
                    self.toast = None;
                }
                // 界面动画整体按设置里的速度倍率缩放。
                let speed = self.ui.animation_speed.max(0.3);
                self.phase = (self.phase + 0.38 * speed) % std::f32::consts::TAU;
                // 加载动画按真实时间推进，避免不同页面帧率不同导致速度不一致。
                self.clock = (self.clock + dt * speed) % 600.0;
                // 光效淡入快、淡出慢，加载完成时进度条不会硬切出来。
                let fade_speed = if self.player.loading {
                    1.0 / 0.18
                } else {
                    1.0 / 0.34
                };
                let target = if self.player.loading { 1.0 } else { 0.0 };
                self.load_mix = approach(self.load_mix, target, fade_speed * speed * dt);
                // 页面切换淡入：约 0.32 秒从遮罩中浮现。
                self.screen_fade = approach(self.screen_fade, 0.0, dt * speed / 0.32);
                // 首页内容区切换略快一些。
                self.view_fade = approach(self.view_fade, 0.0, dt * speed / 0.26);
                // 播放页底部「曲目信息 + 控制台」默认收起，只有指针移到窗口
                // 底部中央的悬停区才展开；指针一旦离开就重新收起。
                let hover = self.pointer_known
                    && views::player::pointer_hovers_dock(self.pointer, self.viewport);
                let expanded = self.screen == Screen::NowPlaying && (hover || self.dock_pinned);
                // 展开 / 收起都走一段 0.16 秒的滑出过渡（受界面动画速度缩放）。
                let dock_target = if expanded { 1.0 } else { 0.0 };
                self.dock_mix = approach(self.dock_mix, dock_target, dt * speed / 0.16);
                // 滚动条：滚动中淡入，停手 0.6 秒后淡出。
                for (index, fade) in self.scroll_fade.iter_mut().enumerate() {
                    let target = if scroll_recent(self.scroll_touched[index], now) {
                        1.0
                    } else {
                        0.0
                    };
                    let rate = if target > *fade {
                        SCROLL_FADE_IN
                    } else {
                        SCROLL_FADE_OUT
                    };
                    *fade = approach(*fade, target, dt * speed * rate);
                }
                let player_task = {
                    self.flush_seek();
                    self.refresh_player()
                };
                // 告诉场景当前画布范围：画布铺满窗口，粒子飞出屏幕才移除。
                let stage = views::player::stage_metrics(self.viewport, &self.visual);
                self.ring.set_view(crate::ring::ViewBounds {
                    width: self.viewport.width,
                    height: self.viewport.height,
                    center: [stage.center.x, stage.center.y],
                    radius_outer: crate::viz::outer_radius(stage.size, &self.visual),
                });
                // 环形频谱的旋转、拖尾与粒子都按真实帧间隔推进。
                self.ring.update(dt, &self.player.spectrum, &self.visual);
                self.maybe_persist_player();
                // 透明度滑杆拖动能触发大量变更：停手 0.25 秒后再落盘。
                if let Some(at) = self.ui_save_at {
                    if now.duration_since(at) >= Duration::from_millis(250) {
                        self.flush_ui_settings();
                    }
                }

                // QA 钩子：MELO_SCREENSHOT=/tmp/melo.png 时等首帧稳定后截图并退出。
                if let (Some(at), Some(id)) = (self.screenshot_at, self.window) {
                    if Instant::now() >= at {
                        self.screenshot_at = None;
                        return window::screenshot(id).map(Message::ScreenshotReady);
                    }
                }

                player_task
            }
            Message::PointerMoved(position) => {
                self.pointer = position;
                self.pointer_known = true;
                Task::none()
            }
            Message::CookieChanged(value) => {
                self.cookie_input = value;
                Task::none()
            }
            Message::CookieSubmit => self.cookie_login(),
            Message::OpenLogin => {
                self.open_netease_login();
                Task::none()
            }
            Message::PasteCookie => iced::clipboard::read().map(Message::CookiePasted),
            Message::CookiePasted(result) => {
                match result {
                    Some(text) => {
                        let cookies = parse_cookie_input(&text);
                        if cookies
                            .get("MUSIC_U")
                            .map(|value| !value.is_empty())
                            .unwrap_or(false)
                        {
                            // 回填成规范的单行 Cookie，方便用户核对后再提交。
                            self.cookie_input = cookie_header(&cookies);
                            self.login_error = None;
                        } else {
                            self.login_error =
                                Some("剪贴板里没有找到 MUSIC_U，请重新复制。".to_string());
                        }
                    }
                    None => {
                        self.login_error = Some("无法读取剪贴板，请手动粘贴 Cookie。".to_string());
                    }
                }
                Task::none()
            }
            Message::SessionChecked(result) => match result {
                Ok(Some(profile)) => self.finish_login(profile),
                Ok(None) => {
                    // 本地票据已失效：清掉后在首页内容区打开登录面板。
                    self.client.clear_cookies();
                    self.session.clear();
                    self.persist();
                    self.profile = None;
                    self.avatar = None;
                    self.avatar_busy = false;
                    self.reset_home_state();
                    // 哔哩哔哩仍有效：直接回收藏夹，不强迫重新登录网易云。
                    if self.bili_session.is_logged_in() {
                        self.bili_page = true;
                    } else {
                        self.open_netease_login();
                    }
                    Task::none()
                }
                Err(error) => {
                    self.restore_error = Some(error);
                    // 已经按本地快照进了首页：校验失败就退回恢复页，方便重试。
                    if self.screen == Screen::Home {
                        self.go(Screen::Restoring);
                    }
                    Task::none()
                }
            },
            Message::ProfileLoaded(result) => match result {
                Ok(Some(profile)) => self.finish_login(profile),
                Ok(None) => {
                    self.restore_error = Some("登录状态尚未生效，可以重试或重新登录。".to_string());
                    Task::none()
                }
                Err(error) => {
                    self.restore_error = Some(error);
                    Task::none()
                }
            },
            Message::CookieChecked(result) => {
                self.cookie_busy = false;
                match result {
                    Ok(Some(profile)) => self.finish_login(profile),
                    Ok(None) => {
                        self.client.clear_cookies();
                        self.login_error = Some("Cookie 无效或已经过期，请重新复制。".to_string());
                        Task::none()
                    }
                    Err(error) => {
                        self.login_error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::AvatarLoaded(result) => {
                self.avatar_busy = false;
                if let Ok(bytes) = result {
                    self.avatar = Some(image::Handle::from_bytes(bytes));
                }
                Task::none()
            }
            Message::StatsLoaded(result) => {
                if let Ok(Some(stats)) = result {
                    if let Some(profile) = &mut self.profile {
                        profile.level = stats.level;
                        profile.follows = stats.follows;
                        profile.followeds = stats.followeds;
                        profile.playlist_count = stats.playlist_count;
                        profile.event_count = stats.event_count;
                        self.session.profile = Some(profile.clone());
                        self.persist();
                    }
                }
                Task::none()
            }
            Message::SearchChanged(value) => {
                self.search.input = value;
                Task::none()
            }
            Message::SearchSubmit => self.submit_search(),
            Message::ClearSearch => {
                self.search.submitted = false;
                self.search.query.clear();
                self.search.results.clear();
                self.search.total = 0;
                self.search.offset = 0;
                self.search.more_busy = false;
                self.search.error = None;
                self.search.busy = false;
                Task::none()
            }
            Message::SearchLoaded(result) => {
                self.search.busy = false;
                if self.screen != Screen::Home {
                    return Task::none();
                }
                match result {
                    Ok(page) => {
                        self.search.error = None;
                        // 有些响应不带 songCount：至少保证不小于首页条数。
                        self.search.total = page.total.max(page.songs.len() as u64);
                        self.search.offset = page.songs.len() as u32;
                        let covers = page.songs.iter().map(|song| song.cover(96)).collect();
                        #[cfg(debug_assertions)]
                        let first = page.songs.first().cloned();
                        self.search.results = page.songs;
                        let task = self.ensure_covers(covers);

                        // 开发辅助：MELO_SEARCH + MELO_AUTOPLAY=1 直接播放第一条结果。
                        #[cfg(debug_assertions)]
                        let task = match first {
                            Some(song) if std::env::var("MELO_AUTOPLAY").as_deref() == Ok("1") => {
                                if std::env::var("MELO_SHOW_PLAYER").as_deref() == Ok("1") {
                                    self.go(Screen::NowPlaying);
                                }
                                Task::batch([task, self.play_from(vec![song], 0)])
                            }
                            _ => task,
                        };

                        task
                    }
                    Err(error) => {
                        self.search.error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::SearchMore => self.search_more(),
            Message::SearchMoreLoaded(result) => {
                self.search.more_busy = false;
                if self.screen != Screen::Home {
                    return Task::none();
                }
                match result {
                    Ok(page) => {
                        self.search.error = None;
                        self.search.offset =
                            self.search.offset.saturating_add(page.songs.len() as u32);
                        let loaded = self.search.results.len() as u64 + page.songs.len() as u64;
                        self.search.total = page.total.max(loaded);
                        let known: HashSet<u64> =
                            self.search.results.iter().map(|song| song.id).collect();
                        let fresh: Vec<Song> = page
                            .songs
                            .into_iter()
                            .filter(|song| !known.contains(&song.id))
                            .collect();
                        if fresh.is_empty() {
                            // 接口把同一页又返回了一次：收紧总数，别让按钮永远可点。
                            self.search.total = self.search.results.len() as u64;
                            return Task::none();
                        }
                        let covers = fresh.iter().map(|song| song.cover(96)).collect();
                        self.search.results.extend(fresh);
                        self.ensure_covers(covers)
                    }
                    Err(_) => {
                        self.show_toast("加载更多失败，请稍后重试");
                        Task::none()
                    }
                }
            }
            Message::LibraryLoaded(result) => {
                self.library.busy = false;
                if self.screen != Screen::Home {
                    return Task::none();
                }
                match result {
                    Ok(playlists) => {
                        self.library.error = None;
                        let covers = playlists.iter().map(|item| item.cover(300)).collect();
                        self.library.playlists = playlists;
                        // 记住这次的结果，下次启动先摆出来再刷新。
                        self.persist_library();
                        self.ensure_covers(covers)
                    }
                    Err(error) => {
                        self.library.error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::ReloadLibrary => self.reload_library(),
            Message::OpenPlaylist(id) => {
                if id == 0 {
                    self.open_liked()
                } else {
                    self.open_playlist(id)
                }
            }
            Message::PlaylistLoaded(result) => {
                let Some(state) = &mut self.playlist else {
                    return Task::none();
                };
                state.busy = false;
                match result {
                    Ok(detail) => {
                        state.error = None;
                        let missing = detail.missing_track_ids();
                        state.filling = !missing.is_empty();
                        let id = state.id;

                        // 只预取首屏可见行的封面；其余随滚动加载，避免上千个请求。
                        let (first, last) =
                            playlist_window(self.viewport.height, 0.0, detail.tracks.len());
                        let mut covers = Vec::with_capacity(last - first + 1);
                        covers.push(detail.cover(420));
                        covers.extend(detail.tracks[first..last].iter().map(|song| song.cover(96)));
                        state.detail = Some(detail);
                        let covers = self.ensure_covers(covers);

                        if missing.is_empty() {
                            covers
                        } else {
                            Task::batch([
                                covers,
                                Task::perform(
                                    fill_playlist_tracks(self.client.clone(), missing),
                                    move |result| Message::PlaylistTracksFilled { id, result },
                                ),
                            ])
                        }
                    }
                    Err(error) => {
                        state.error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::PlaylistTracksFilled { id, result } => {
                let Some(state) = &mut self.playlist else {
                    return Task::none();
                };
                // 用户可能已经换了一个歌单。
                if state.id != id {
                    return Task::none();
                }
                state.filling = false;
                if let (Ok(songs), Some(detail)) = (result, &mut state.detail) {
                    let known: HashSet<u64> = detail.tracks.iter().map(|song| song.id).collect();
                    detail
                        .tracks
                        .extend(songs.into_iter().filter(|song| !known.contains(&song.id)));
                }
                // 补齐的歌曲可能正好落在当前搜索结果里，顺手把可见行的封面拉下来。
                let offset = self.playlist_scroll;
                self.ensure_playlist_covers(offset)
            }
            Message::PlaylistScrolled(offset) => {
                self.touch_scroll(ScrollArea::TrackList);
                self.playlist_scroll = offset;
                self.ensure_playlist_covers(offset)
            }
            Message::PlaylistSearchChanged(value) => {
                if let Some(state) = &mut self.playlist {
                    state.search = value;
                }
                self.playlist_scroll = 0.0;
                Task::batch([
                    self.ensure_playlist_search_coverage(),
                    self.ensure_playlist_covers(0.0),
                    scroll_playlist_to_top(),
                ])
            }
            Message::PlaylistSearchSubmit => {
                self.playlist_scroll = 0.0;
                Task::batch([
                    self.ensure_playlist_search_coverage(),
                    self.ensure_playlist_covers(0.0),
                    scroll_playlist_to_top(),
                ])
            }
            Message::PlaylistSearchClear => {
                if let Some(state) = &mut self.playlist {
                    state.search.clear();
                }
                self.playlist_scroll = 0.0;
                Task::batch([self.ensure_playlist_covers(0.0), scroll_playlist_to_top()])
            }
            Message::ClosePlaylist => {
                self.playlist = None;
                // 离开设置页时立刻把待落盘的偏好写下去，不用等去抖动计时器。
                if self.settings_page {
                    self.flush_ui_settings();
                }
                self.settings_page = false;
                self.netease_login_page = false;
                self.about_page = false;
                self.search_page = false;
                self.daily_page = false;
                self.fm_page = false;
                self.bili_page = false;
                self.artist = None;
                self.view_fade = 1.0;
                Task::none()
            }
            Message::CoverLoaded {
                url,
                cached,
                result,
            } => {
                if let Ok(cover) = result {
                    // 网络刷新已经先把新图放进去了，晚到的缓存结果就不要再盖回去。
                    if cached && self.cover_fresh.contains(&url) {
                        return Task::none();
                    }
                    if !cached {
                        self.cover_fresh.insert(url.clone());
                    }
                    if let Some(circle) = cover.circle {
                        self.circle_covers
                            .insert(url.clone(), image::Handle::from_bytes(circle));
                    }
                    self.covers
                        .insert(url, image::Handle::from_bytes(cover.bytes));
                }
                Task::none()
            }
            Message::PlayPause => {
                let state = self.player.engine.snapshot().state;
                match state {
                    nm_audio::State::Playing | nm_audio::State::Paused => {
                        self.player.engine.toggle();
                        self.mark_player_dirty();
                        Task::none()
                    }
                    // 恢复后第一次播放：从上次退出的进度继续。
                    _ => {
                        self.player.auto_skips = 0;
                        let resume = self.player.pending_resume.take().unwrap_or(Duration::ZERO);
                        self.begin_current(resume)
                    }
                }
            }
            Message::PlaySearchTrack(index) => {
                let songs = self.search.results.clone();
                self.play_from(songs, index)
            }
            Message::PlayDailyTrack(index) => {
                let songs = self.daily.songs.clone();
                self.play_from(songs, index)
            }
            Message::PlayFmTrack(index) => {
                let songs = self.fm.songs.clone();
                self.play_fm_from(songs, index)
            }
            Message::PlayAllFm => {
                let songs = self.fm.songs.clone();
                self.play_fm_from(songs, 0)
            }
            Message::PlayPlaylistTrack(index) => {
                let tracks = self
                    .playlist
                    .as_ref()
                    .and_then(|state| state.detail.as_ref())
                    .map(|detail| detail.tracks.clone())
                    .unwrap_or_default();
                self.play_from(tracks, index)
            }
            Message::ShufflePlaylist => {
                let tracks = self
                    .playlist
                    .as_ref()
                    .and_then(|state| state.detail.as_ref())
                    .map(|detail| detail.tracks.clone())
                    .unwrap_or_default();
                if tracks.is_empty() {
                    return Task::none();
                }
                if !self.player.mode.shuffle {
                    self.player.mode.shuffle = true;
                    self.mark_player_dirty();
                }
                self.play_from(tracks, 0)
            }
            Message::EnqueueSearchTrack(index) => {
                let song = self.search.results.get(index).cloned();
                match song {
                    Some(song) => self.enqueue_next(song),
                    None => Task::none(),
                }
            }
            Message::EnqueueDailyTrack(index) => {
                let song = self.daily.songs.get(index).cloned();
                match song {
                    Some(song) => self.enqueue_next(song),
                    None => Task::none(),
                }
            }
            Message::EnqueueFmTrack(index) => {
                let song = self.fm.songs.get(index).cloned();
                match song {
                    Some(song) => self.enqueue_next(song),
                    None => Task::none(),
                }
            }
            Message::EnqueuePlaylistTrack(index) => {
                let song = self
                    .playlist
                    .as_ref()
                    .and_then(|state| state.detail.as_ref())
                    .and_then(|detail| detail.tracks.get(index))
                    .cloned();
                match song {
                    Some(song) => self.enqueue_next(song),
                    None => Task::none(),
                }
            }
            Message::NextTrack => self.skip_next(),
            Message::PrevTrack => self.previous_track(),
            Message::SeekTo(fraction) => {
                let fraction = fraction.clamp(0.0, 1.0);
                if self.player.duration > Duration::ZERO {
                    let target = self.player.duration.mul_f32(fraction);
                    let now = Instant::now();
                    self.player.position = target;
                    self.player.seek_pending = Some((target, now));
                    // 拖动期间只记录目标，等指针停稳再由 `flush_seek` 真正 seek 一次。
                    self.player.seek_commit = Some((target, now));
                    // 恢复后还没开始播放：把恢复点一起拖到新位置，下次播放从这里开始。
                    if self.player.pending_resume.is_some() {
                        self.player.pending_resume = Some(target);
                    }
                    self.mark_player_dirty();
                }
                Task::none()
            }
            Message::VolumeScrolled(delta) => {
                // 静音状态下滚轮：先恢复上一次音量，再在它基础上加减。
                let base = self
                    .player
                    .muted_volume
                    .take()
                    .unwrap_or(self.player.volume);
                self.player.volume = volume_after_scroll(base, delta);
                self.player.engine.set_volume(self.player.volume);
                self.mark_player_dirty();
                Task::none()
            }
            Message::ToggleMute => {
                toggle_mute(&mut self.player.volume, &mut self.player.muted_volume);
                self.player.engine.set_volume(self.player.volume);
                self.mark_player_dirty();
                Task::none()
            }
            Message::OpenNowPlaying => {
                if self.player.current.is_some() {
                    // 刚进入页面时一定是收起状态：等指针真的移到窗口底部
                    // 中央，信息与控制台才会滑出。
                    self.dock_mix = 0.0;
                    self.pointer_known = false;
                    self.go(Screen::NowPlaying);
                }
                Task::none()
            }
            Message::CloseNowPlaying => {
                self.show_settings = false;
                self.show_quality = false;
                self.show_queue = false;
                self.go(Screen::Home);
                Task::none()
            }
            Message::ToggleQueue => {
                self.show_queue = !self.show_queue;
                if self.show_queue {
                    self.show_settings = false;
                    self.show_quality = false;
                    return self.restore_queue_view();
                }
                Task::none()
            }
            Message::QueuePlay(index) => {
                if index < self.player.queue.len() {
                    self.player.auto_skips = 0;
                    self.player.index = index;
                    self.start_current()
                } else {
                    Task::none()
                }
            }
            Message::QueueRemove(index) => {
                let mut current = self.player.index;
                let change = remove_queue_entry(&mut self.player.queue, &mut current, index);
                self.player.index = current;
                drop_history_entry(&mut self.player.history, index);
                self.mark_player_dirty();
                match change {
                    QueueChange::Keep => Task::none(),
                    QueueChange::Play => self.start_current(),
                    QueueChange::Stop => {
                        self.stop_and_clear();
                        Task::none()
                    }
                }
            }
            Message::QueueClear => {
                self.player.queue.clear();
                self.player.index = 0;
                self.player.history.clear();
                self.queue_scroll = 0.0;
                self.stop_and_clear();
                Task::none()
            }
            Message::ToggleShuffle => {
                self.player.mode.shuffle = !self.player.mode.shuffle;
                self.mark_player_dirty();
                Task::none()
            }
            Message::CycleRepeat => {
                self.player.mode.repeat = self.player.mode.repeat.next();
                self.mark_player_dirty();
                Task::none()
            }
            Message::QueueScrolled(offset) => {
                self.touch_scroll(ScrollArea::Queue);
                self.queue_scroll = offset.max(0.0);
                self.ensure_queue_covers(self.queue_scroll)
            }
            Message::Scrolled(area) => {
                self.touch_scroll(area);
                Task::none()
            }
            Message::SetQuality(quality) => {
                self.show_quality = false;
                if self.player.quality == quality {
                    return Task::none();
                }
                self.player.quality = quality;
                self.mark_player_dirty();
                self.reload_after_quality_change()
            }
            Message::ToggleQualityMenu => {
                self.show_quality = !self.show_quality;
                if self.show_quality {
                    self.show_settings = false;
                    self.show_queue = false;
                    self.liked_transfer = None;
                }
                Task::none()
            }
            Message::OpenLikedTransfer(kind) => {
                // 同一个弹窗只保留一份，打开时把其它浮层收起来。
                self.show_settings = false;
                self.show_quality = false;
                self.show_queue = false;
                self.liked_transfer = Some(LikedTransfer::new(kind, &self.paths));
                Task::none()
            }
            Message::LikedTransferPath(path) => {
                if let Some(transfer) = &mut self.liked_transfer {
                    transfer.path = path;
                    transfer.error = None;
                }
                Task::none()
            }
            Message::CloseLikedTransfer => {
                self.liked_transfer = None;
                Task::none()
            }
            Message::ConfirmLikedTransfer => self.confirm_liked_transfer(),
            Message::ToggleSettings => {
                self.show_settings = !self.show_settings;
                if self.show_settings {
                    self.show_quality = false;
                    self.show_queue = false;
                }
                Task::none()
            }
            Message::OpenSettings => {
                // 设置是独立页面：打开时收起所有浮层，回到首页内容区。
                self.show_quality = false;
                self.show_queue = false;
                self.show_settings = false;
                self.settings_page = true;
                self.about_page = false;
                self.search_page = false;
                self.daily_page = false;
                self.fm_page = false;
                self.bili_page = false;
                self.artist = None;
                self.view_fade = 1.0;
                Task::none()
            }
            Message::OpenAbout => {
                self.show_quality = false;
                self.show_queue = false;
                self.show_settings = false;
                self.settings_page = false;
                self.about_page = true;
                self.search_page = false;
                self.daily_page = false;
                self.fm_page = false;
                self.bili_page = false;
                self.artist = None;
                self.view_fade = 1.0;
                Task::none()
            }
            Message::OpenProjectUrl => {
                // 项目只面向 Linux 桌面，直接交给系统的 URL 处理器。
                match Command::new("xdg-open").arg(PROJECT_URL).spawn() {
                    Ok(mut child) => {
                        // 等子进程退出并回收，避免应用长时间运行时留下僵尸进程。
                        std::thread::spawn(move || {
                            let _ = child.wait();
                        });
                        Task::none()
                    }
                    Err(error) => {
                        self.show_toast(format!(
                            "无法打开浏览器（{error}），请手动访问 {PROJECT_URL}"
                        ));
                        Task::none()
                    }
                }
            }
            Message::AcceptDisclaimer => {
                if !self.ui.disclaimer_accepted {
                    self.ui.disclaimer_accepted = true;
                    // 只确认一次：立即落盘，不等滑杆之类的去抖动保存。
                    if let Err(error) = self.ui.save(&self.paths.settings_file()) {
                        self.storage_error = Some(error);
                    }
                }
                Task::none()
            }
            Message::DisclaimerBlocked => Task::none(),
            Message::OpenSearch => {
                if self.profile.is_none() {
                    self.show_toast("这个页面需要网易云账号，请先登录网易云。".to_string());
                    return Task::none();
                }
                self.show_quality = false;
                self.show_queue = false;
                self.show_settings = false;
                self.playlist = None;
                self.settings_page = false;
                self.about_page = false;
                self.search_page = true;
                self.daily_page = false;
                self.fm_page = false;
                self.bili_page = false;
                self.artist = None;
                self.view_fade = 1.0;
                Task::none()
            }
            Message::OpenDaily => {
                if self.profile.is_none() {
                    self.show_toast("这个页面需要网易云账号，请先登录网易云。".to_string());
                    return Task::none();
                }
                self.show_quality = false;
                self.show_queue = false;
                self.show_settings = false;
                self.playlist = None;
                self.settings_page = false;
                self.about_page = false;
                self.search_page = false;
                self.daily_page = true;
                self.fm_page = false;
                self.bili_page = false;
                self.artist = None;
                self.view_fade = 1.0;
                // 推荐按天更新：每次进入都重新取一次。
                self.load_daily()
            }
            Message::OpenFm => {
                if self.profile.is_none() {
                    self.show_toast("这个页面需要网易云账号，请先登录网易云。".to_string());
                    return Task::none();
                }
                self.show_quality = false;
                self.show_queue = false;
                self.show_settings = false;
                self.playlist = None;
                self.settings_page = false;
                self.about_page = false;
                self.search_page = false;
                self.daily_page = false;
                self.fm_page = true;
                self.bili_page = false;
                self.artist = None;
                self.view_fade = 1.0;
                // 已经拿过这一批就保留，切回来不用重新请求；换一批用刷新按钮。
                if self.fm.songs.is_empty() {
                    self.load_fm()
                } else {
                    Task::none()
                }
            }
            Message::OpenBilibili => {
                self.show_quality = false;
                self.show_queue = false;
                self.show_settings = false;
                self.playlist = None;
                self.settings_page = false;
                self.about_page = false;
                self.search_page = false;
                self.daily_page = false;
                self.fm_page = false;
                self.artist = None;
                self.bili_page = true;
                self.netease_login_page = false;
                self.view_fade = 1.0;

                let mut tasks = Vec::new();
                let face = nm_api::sized_cover(&self.bili_session.face, 600);
                if !face.is_empty() {
                    tasks.push(self.ensure_covers(vec![face]));
                }
                // 已登录但还没拉过收藏夹：进入页面时自动取一次。
                if self.bili_session.is_logged_in() && self.bili_state.folders.is_empty() {
                    tasks.push(self.load_bili_folders());
                }
                Task::batch(tasks)
            }
            Message::BiliCookieChanged(value) => {
                self.bili_state.cookie_input = value;
                self.bili_state.connect_error = None;
                Task::none()
            }
            Message::BiliPasteCookie => iced::clipboard::read().map(Message::BiliCookiePasted),
            Message::BiliCookiePasted(result) => {
                match result {
                    Some(text) if !text.trim().is_empty() => {
                        // 保留原文：`parse_bilibili_cookie_input` 自己会处理
                        // 整段 Cookie / DevTools 表格行 / 裸 SESSDATA。
                        self.bili_state.cookie_input = text.trim().to_string();
                        self.bili_state.connect_error = None;
                    }
                    _ => {
                        self.bili_state.connect_error =
                            Some("无法读取剪贴板，请手动粘贴 Cookie。".to_string());
                    }
                }
                Task::none()
            }
            Message::BiliConnect => {
                if self.bili_state.connecting {
                    return Task::none();
                }
                let input = self.bili_state.cookie_input.trim().to_string();
                if input.is_empty() {
                    self.bili_state.connect_error =
                        Some("请粘贴哔哩哔哩 Cookie 或 SESSDATA。".to_string());
                    return Task::none();
                }
                self.bili_state.connecting = true;
                self.bili_state.connect_error = None;
                Task::perform(
                    connect_bilibili(self.bili.clone(), input),
                    Message::BiliConnected,
                )
            }
            Message::BiliConnected(result) => {
                self.bili_state.connecting = false;
                match result {
                    Ok(user) => {
                        self.bili_session.cookies = self.bili.snapshot_cookies();
                        self.bili_session.mid = user.mid;
                        self.bili_session.name = user.name.clone();
                        self.bili_session.face = user.face.clone();
                        if let Err(error) = self.bili_session.save(&self.paths.bilibili_file()) {
                            self.storage_error = Some(error);
                        }
                        self.bili_state.cookie_input.clear();
                        self.bili_state.connect_error = None;
                        self.bili_state.folders.clear();
                        self.bili_state.folders_error = None;
                        self.bili_state.folders_busy = false;

                        // 连接成功：关掉登录面板，回到哔哩哔哩分区；
                        // 首页已有内容时保留（侧边栏一直存在，不需要重建）。
                        if !self.session.is_logged_in() {
                            self.profile = None;
                            self.avatar = None;
                            self.avatar_busy = false;
                        }
                        self.netease_login_page = false;
                        self.bili_page = true;
                        self.view_fade = 1.0;
                        if self.screen != Screen::NowPlaying {
                            self.go(Screen::Home);
                        }

                        let mut tasks = Vec::new();
                        if let Some(url) = self.player.current.as_ref().map(|song| song.cover(600))
                        {
                            self.prime_cached_covers(vec![url.clone()]);
                            tasks.push(self.ensure_covers(vec![url]));
                        }
                        if !user.face.is_empty() {
                            tasks.push(
                                self.ensure_covers(vec![nm_api::sized_cover(&user.face, 600)]),
                            );
                        }
                        tasks.push(self.load_bili_folders());
                        Task::batch(tasks)
                    }
                    Err(error) => {
                        self.bili_state.connect_error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::BiliFoldersLoaded(result) => {
                self.bili_state.folders_busy = false;
                // 退出登录后到达的旧响应直接丢弃。
                if !self.bili_session.is_logged_in() {
                    return Task::none();
                }
                match result {
                    Ok(folders) => {
                        self.bili_state.folders_error = None;
                        let covers = folders.iter().map(|folder| folder.cover(300)).collect();
                        let mut tasks = vec![self.ensure_covers(covers)];

                        // 收藏夹列表接口不返回封面：为空时去拉第一页的第一条视频。
                        for folder in &folders {
                            if folder.cover.is_empty() && folder.media_count > 0 {
                                let id = folder.id;
                                tasks.push(Task::perform(
                                    fetch_bili_folder_cover(self.bili.clone(), id),
                                    move |cover| Message::BiliFolderCover { id, cover },
                                ));
                            }
                        }

                        self.bili_state.folders = folders;
                        Task::batch(tasks)
                    }
                    Err(error) => {
                        self.bili_state.folders_error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::BiliFolderCover { id, cover } => {
                if cover.is_empty() {
                    return Task::none();
                }
                let Some(folder) = self
                    .bili_state
                    .folders
                    .iter_mut()
                    .find(|folder| folder.id == id)
                else {
                    return Task::none();
                };
                folder.cover = cover.clone();
                let url = nm_api::sized_cover(&cover, 300);
                self.ensure_covers(vec![url])
            }
            Message::ToggleBiliFolderHidden(id) => {
                self.bili_session.toggle_folder_hidden(id);
                if let Err(error) = self.bili_session.save(&self.paths.bilibili_file()) {
                    self.storage_error = Some(error);
                }
                Task::none()
            }
            Message::ToggleShowHiddenBiliFolders => {
                self.bili_state.show_hidden = !self.bili_state.show_hidden;
                Task::none()
            }
            Message::ReloadBiliFolders => {
                // 手动刷新时允许重试上一次失败 / 仍在进行中的请求。
                self.bili_state.folders_busy = false;
                self.load_bili_folders()
            }
            Message::OpenBiliFolder(id) => self.open_bili_folder(id),
            Message::BiliFolderLoaded { id, page, result } => {
                let Some(state) = self.playlist.as_mut() else {
                    return Task::none();
                };
                if !state.bilibili || state.id != id {
                    return Task::none();
                }
                match result {
                    Ok(page_data) => {
                        let covers = apply_bili_page(state, id, page, page_data);
                        // 窗口很高时一页可能撑不满一屏，滚不动就永远触发不了自动翻页；
                        // 正在搜索时也必须把所有页都取完，结果才能覆盖未加载的歌曲。
                        // 这里直接继续取下一页，直到填满 / 搜完或没有更多。
                        let searching = !state.search.trim().is_empty();
                        let need_more = state.bili_has_more
                            && (searching
                                || state
                                    .detail
                                    .as_ref()
                                    .map_or(0, |detail| detail.tracks.len())
                                    as f32
                                    * views::home::TRACK_ROW_HEIGHT
                                    < self.viewport.height - 220.0);
                        let covers_task = self.ensure_covers(covers);
                        if need_more {
                            if let Some(state) = &mut self.playlist {
                                state.bili_busy = true;
                            }
                            Task::batch([covers_task, self.load_bili_folder(id, page + 1)])
                        } else {
                            covers_task
                        }
                    }
                    Err(error) => {
                        state.bili_busy = false;
                        if page == 1 {
                            state.busy = false;
                            state.error = Some(error);
                        } else {
                            self.show_toast(format!("加载更多失败：{error}"));
                        }
                        Task::none()
                    }
                }
            }
            Message::LoadMoreBiliFolder => {
                let Some(state) = &mut self.playlist else {
                    return Task::none();
                };
                if !state.bilibili || state.bili_busy || !state.bili_has_more {
                    return Task::none();
                }
                state.bili_busy = true;
                let id = state.id;
                let page = state.bili_page + 1;
                self.load_bili_folder(id, page)
            }
            Message::BiliLogout => {
                self.bili.clear_cookies();
                self.bili_session.clear();
                if let Err(error) = self.bili_session.save(&self.paths.bilibili_file()) {
                    self.storage_error = Some(error);
                }
                self.bili_state = BilibiliState::default();
                if self.playlist.as_ref().is_some_and(|state| state.bilibili) {
                    self.playlist = None;
                }
                self.bili_page = true;
                Task::none()
            }
            Message::ReloadFm => self.load_fm(),
            Message::FmLoaded(result) => {
                self.fm.busy = false;
                match result {
                    Ok(songs) => {
                        self.fm.error = None;
                        let covers = songs.iter().map(|song| song.cover(96)).collect();
                        self.fm.songs = songs;
                        self.ensure_covers(covers)
                    }
                    Err(error) => {
                        self.fm.error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::FmMoreLoaded(result) => {
                self.player.fm_fetching = false;
                self.player.loading = false;
                if !self.player.fm_queue {
                    return Task::none();
                }
                match result {
                    Ok(songs) => {
                        let Some(start) = append_unique(&mut self.player.queue, songs) else {
                            // 没有新歌就停下，别让自动下一首每帧都再发请求。
                            self.player.fm_queue = false;
                            return Task::none();
                        };
                        let covers = self.player.queue[start..]
                            .iter()
                            .map(|song| song.cover(96))
                            .collect();
                        self.player.index = start;
                        self.mark_player_dirty();
                        Task::batch([self.start_current(), self.ensure_covers(covers)])
                    }
                    Err(error) => {
                        self.player.fm_queue = false;
                        self.player.error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::OpenArtist(id) => self.open_artist(id),
            Message::CloseArtist => {
                let from_player = self.artist.as_ref().is_some_and(|state| state.from_player);
                self.artist = None;
                self.view_fade = 1.0;
                if from_player && self.player.current.is_some() {
                    self.go(Screen::NowPlaying);
                }
                Task::none()
            }
            Message::ToggleArtistBio => {
                if let Some(state) = &mut self.artist {
                    state.bio_expanded = !state.bio_expanded;
                }
                Task::none()
            }
            Message::ArtistLoaded { id, result } => {
                let Some(state) = self.artist.as_mut() else {
                    return Task::none();
                };
                // 快速切换歌手 / 已经关闭页面：丢弃过期结果。
                if state.id != id {
                    return Task::none();
                }
                state.busy = false;
                match result {
                    Ok((detail, songs)) => {
                        state.error = None;
                        state.total = detail.music_size.max(songs.len() as u64);
                        state.offset = songs.len() as u32;
                        let covers = songs.iter().map(|song| song.cover(96)).collect::<Vec<_>>();
                        state.songs = songs;
                        // 600 的封面会额外生成圆形版本，歌手头像正好用得上。
                        let avatar = nm_api::sized_cover(detail.image(), 600);
                        state.detail = Some(detail);
                        self.ensure_covers(covers.into_iter().chain([avatar]).collect())
                    }
                    Err(error) => {
                        state.error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::ArtistMore => self.artist_more(),
            Message::ArtistMoreLoaded { id, result } => {
                let Some(state) = self.artist.as_mut() else {
                    return Task::none();
                };
                if state.id != id {
                    return Task::none();
                }
                state.more_busy = false;

                let outcome: Result<Vec<String>, ()> = match result {
                    Ok(songs) => {
                        state.offset = state.offset.saturating_add(songs.len() as u32);
                        match append_unique(&mut state.songs, songs) {
                            Some(start) => Ok(state.songs[start..]
                                .iter()
                                .map(|song| song.cover(96))
                                .collect()),
                            None => {
                                // 没有新歌：收紧总数，别让按钮永远可点。
                                state.total = state.songs.len() as u64;
                                Ok(Vec::new())
                            }
                        }
                    }
                    Err(_) => Err(()),
                };

                match outcome {
                    Ok(covers) => self.ensure_covers(covers),
                    Err(()) => {
                        self.show_toast("加载更多失败，请稍后重试");
                        Task::none()
                    }
                }
            }
            Message::PlayArtistTrack(index) => {
                let songs = self
                    .artist
                    .as_ref()
                    .map(|state| state.songs.clone())
                    .unwrap_or_default();
                self.play_from(songs, index)
            }
            Message::EnqueueArtistTrack(index) => {
                let song = self
                    .artist
                    .as_ref()
                    .and_then(|state| state.songs.get(index))
                    .cloned();
                match song {
                    Some(song) => self.enqueue_next(song),
                    None => Task::none(),
                }
            }
            Message::ToggleLike(song) => self.toggle_like(song),
            Message::OpenLiked => self.open_liked(),
            Message::DownloadSong(song) => self.download_song(song),
            Message::RemoveDownload(id) => self.remove_download(id),
            Message::OpenDownloads => self.open_downloads(),
            Message::Downloaded { song, result } => self.downloaded(song, result),
            Message::AskClearDownloads => {
                if self.download_tracks.is_empty() {
                    return Task::none();
                }
                self.confirm_clear_downloads = true;
                Task::none()
            }
            Message::CancelClearDownloads => {
                self.confirm_clear_downloads = false;
                Task::none()
            }
            Message::ClearDownloads => self.clear_downloads(),
            Message::OpenLocalFolders => {
                self.show_quality = false;
                self.show_queue = false;
                self.show_settings = false;
                self.liked_transfer = None;
                self.confirm_clear_downloads = false;
                if self.local_folder_dialog.is_none() {
                    self.local_folder_dialog = Some(LocalFolderDialog::new());
                }
                Task::none()
            }
            Message::CloseLocalFolders => {
                self.local_folder_dialog = None;
                Task::none()
            }
            Message::LocalFolderPath(path) => {
                if let Some(dialog) = &mut self.local_folder_dialog {
                    dialog.path = path;
                    dialog.error = None;
                }
                Task::none()
            }
            Message::AddLocalFolder => self.add_local_folder(),
            Message::RemoveLocalFolder(index) => self.remove_local_folder(index),
            Message::RescanLocalFolders => self.scan_local_folders(),
            Message::LocalFoldersScanned { seq, folders } => {
                self.local_folders_scanned(seq, folders)
            }
            Message::ReloadDaily => self.load_daily(),
            Message::DailyLoaded(result) => {
                self.daily.busy = false;
                match result {
                    Ok(songs) => {
                        self.daily.error = None;
                        let covers = songs.iter().map(|song| song.cover(96)).collect();
                        self.daily.songs = songs;
                        self.ensure_covers(covers)
                    }
                    Err(error) => {
                        self.daily.error = Some(error);
                        Task::none()
                    }
                }
            }
            Message::SetAnimationSpeed(speed) => {
                let speed = speed.clamp(0.3, 3.0);
                if (self.ui.animation_speed - speed).abs() < f32::EPSILON {
                    return Task::none();
                }
                self.ui.animation_speed = speed;
                if let Err(error) = self.ui.save(&self.paths.settings_file()) {
                    self.storage_error = Some(error);
                }
                Task::none()
            }
            Message::SetDockOpacity(opacity) => {
                let (low, high) = crate::ui_settings::DOCK_OPACITY_RANGE;
                let opacity = opacity.clamp(low, high);
                if (self.ui.dock_opacity - opacity).abs() < f32::EPSILON {
                    return Task::none();
                }
                self.ui.dock_opacity = opacity;
                // 拖动中只改内存，由 Tick 做去抖动落盘。
                self.ui_save_at = Some(Instant::now());
                Task::none()
            }
            Message::SetCacheLimit(limit_mb) => {
                // 0 表示关闭；其余值落在 3 GB ~ 10 GB。
                let limit_mb = if limit_mb == 0 {
                    0
                } else {
                    limit_mb.clamp(audio_cache::LIMIT_MIN_MB, audio_cache::LIMIT_MAX_MB)
                };
                if self.ui.cache_limit_mb == limit_mb {
                    return Task::none();
                }
                self.ui.cache_limit_mb = limit_mb;
                {
                    let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
                    cache.set_limit(limit_mb * MB);
                    self.cache_usage = cache.usage();
                }
                if let Err(error) = self.ui.save(&self.paths.settings_file()) {
                    self.storage_error = Some(error);
                }
                Task::none()
            }
            Message::ClearCache => {
                {
                    let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
                    cache.clear();
                    self.cache_usage = cache.usage();
                }
                // 封面与首页快照也算缓存：一并清掉，下次启动重新拉取。
                // 界面上的图片句柄先留着，避免点完按钮整个页面闪回文字占位。
                cover_cache::clear(&self.paths.cover_cache_dir());
                let _ = std::fs::remove_file(self.paths.library_file());
                Task::none()
            }
            Message::VisualValue(field, value) => {
                self.visual.apply_normalized(field, value);
                self.player.processor.set_gains(self.visual.gains());
                self.persist_visual();
                Task::none()
            }
            Message::VisualToggle(toggle) => {
                match toggle {
                    VisualToggle::Rotate => self.visual.rotate = !self.visual.rotate,
                    VisualToggle::Links => self.visual.links = !self.visual.links,
                    VisualToggle::Particles => self.visual.particles = !self.visual.particles,
                    VisualToggle::Burst => self.visual.burst = !self.visual.burst,
                    VisualToggle::HighFps => self.visual.high_fps = !self.visual.high_fps,
                }
                self.persist_visual();
                Task::none()
            }
            Message::VisualPalette(palette) => {
                self.visual.palette = palette;
                self.persist_visual();
                Task::none()
            }
            Message::ResetVisual => {
                self.visual = VisualizerConfig::default();
                self.player.processor.set_gains(self.visual.gains());
                self.persist_visual();
                Task::none()
            }
            Message::TrackLoaded { id, seq, result } => {
                // 用户可能已经切歌或换了音质，过期的下载结果直接丢弃。
                if seq != self.player.load_seq
                    || self.player.current.as_ref().map(|song| song.id) != Some(id)
                {
                    return Task::none();
                }
                self.player.loading = false;
                match result {
                    Ok(track) => {
                        self.player.auto_skips = 0;
                        self.player.loaded_quality = track.label;
                        self.player.error = None;
                        self.player.engine.load(track.bytes);
                        // 异步任务里可能刚写进缓存，重新读一次占用。
                        self.cache_usage = self
                            .cache
                            .lock()
                            .unwrap_or_else(|error| error.into_inner())
                            .usage();
                        // 恢复场景：音频真正加载完成后再 seek，命令顺序能保证它生效。
                        if let Some(position) = self.player.pending_resume.take() {
                            self.player.seek_commit = None;
                            self.player.engine.seek(position);
                            self.player.position = position;
                            self.player.seek_pending = Some((position, Instant::now()));
                        }
                        // 换音质前是暂停状态：加载完不要自己播起来。
                        if std::mem::take(&mut self.player.pause_after_load) {
                            self.player.engine.pause();
                        }
                    }
                    // VIP / 购买 / 无版权：不报错，直接跳下一首并弹一个悬浮提示。
                    Err(TrackError::Skipped(reason)) => return self.auto_skip(reason),
                    Err(TrackError::Failed(error)) => {
                        self.player.auto_skips = 0;
                        self.player.error = Some(error);
                    }
                }
                Task::none()
            }
            Message::ScreenshotReady(screenshot) => {
                if let Some(path) = self.screenshot_path.take() {
                    let width = screenshot.size.width;
                    let height = screenshot.size.height;
                    eprintln!(
                        "screenshot: {}x{} @ {}x",
                        width, height, screenshot.scale_factor
                    );
                    if let Err(error) = ::image::save_buffer(
                        &path,
                        &screenshot.bytes,
                        width,
                        height,
                        ::image::ColorType::Rgba8,
                    ) {
                        eprintln!("截图保存失败：{error}");
                    }
                }
                iced::exit()
            }
            Message::RetrySession => {
                self.restore_error = None;
                Task::perform(load_account(self.client.clone()), Message::ProfileLoaded)
            }
            Message::BackToLogin | Message::Logout => self.sign_out(),
        }
    }

    fn cookie_login(&mut self) -> Task<Message> {
        let cookies = parse_cookie_input(&self.cookie_input);
        if !cookies
            .get("MUSIC_U")
            .map(|value| !value.is_empty())
            .unwrap_or(false)
        {
            self.login_error = Some(
                "请粘贴包含 MUSIC_U 的 Cookie；从开发者工具整行 / 整表复制时请点「粘贴」按钮。"
                    .to_string(),
            );
            return Task::none();
        }

        self.client.set_cookies(cookies);
        self.cookie_busy = true;
        self.login_error = None;

        Task::perform(load_account(self.client.clone()), Message::CookieChecked)
    }

    fn finish_login(&mut self, profile: Profile) -> Task<Message> {
        // 同一账号再次校验时（启动直接进首页的路径），首页已经用快照摆好了，
        // 不要再清空一次，否则封面会先闪成文字占位。
        let keep_home = self
            .profile
            .as_ref()
            .map(|cached| cached.user_id == profile.user_id)
            .unwrap_or(false);

        self.session.cookies = self.client.snapshot_cookies();
        self.session.profile = Some(profile.clone());
        self.persist();

        self.profile = Some(profile.clone());
        self.avatar = None;
        self.avatar_busy = true;
        // 启动后已经直接进了首页：用户可能已经点开正在播放页，不要把人拽回来。
        if self.screen != Screen::NowPlaying {
            self.go(Screen::Home);
        }
        self.login_error = None;
        self.restore_error = None;
        self.cookie_busy = false;
        self.cookie_input.clear();
        self.netease_login_page = false;
        if keep_home {
            self.library.error = None;
        } else {
            self.reset_home_state();
        }

        // 先把上次的歌单与头像按本地缓存摆出来，网络刷新在后台进行。
        self.restore_library_cache();
        self.prime_cached_avatar();

        let mut tasks = Vec::new();

        // 登录会清空封面缓存，恢复出来的当前歌曲要重新拉一次封面。
        if let Some(url) = self.player.current.as_ref().map(|song| song.cover(600)) {
            self.prime_cached_covers(vec![url.clone()]);
            tasks.push(self.ensure_covers(vec![url]));
        }

        let url = profile.avatar_with_size(320);
        if url.is_empty() {
            self.avatar_busy = false;
        } else {
            tasks.push(Task::perform(
                load_avatar(self.client.clone(), self.paths.cover_cache_dir(), url),
                Message::AvatarLoaded,
            ));
        }

        if self.demo {
            self.library.playlists = demo_playlists();
            let covers = self
                .library
                .playlists
                .iter()
                .map(|item| item.cover(300))
                .collect();
            tasks.push(self.ensure_covers(covers));
        } else {
            tasks.push(self.reload_library());
            tasks.push(Task::perform(
                load_stats(self.client.clone(), profile.user_id),
                Message::StatsLoaded,
            ));
        }

        // 开发辅助：登录后直接打开本地收藏页。
        #[cfg(debug_assertions)]
        if std::env::var("MELO_OPEN_LIKED").as_deref() == Ok("1") {
            tasks.push(self.open_liked());
        }

        // 开发辅助：登录后直接打开指定歌单，便于检查真实数据布局。
        #[cfg(debug_assertions)]
        if let Ok(id) = std::env::var("MELO_OPEN_PLAYLIST") {
            if let Ok(id) = id.parse::<u64>() {
                tasks.push(self.open_playlist(id));
            }
        }

        // 开发辅助：登录后直接打开每日推荐，便于检查真实数据布局。
        #[cfg(debug_assertions)]
        if std::env::var("MELO_OPEN_DAILY").as_deref() == Ok("1") {
            self.daily_page = true;
            self.view_fade = 1.0;
            tasks.push(self.load_daily());
        }

        // 开发辅助：登录后直接打开私人 FM，便于检查真实数据布局。
        #[cfg(debug_assertions)]
        if std::env::var("MELO_OPEN_FM").as_deref() == Ok("1") {
            self.fm_page = true;
            self.view_fade = 1.0;
            tasks.push(self.load_fm());
        }

        // 开发辅助：登录后直接打开指定歌手页。
        #[cfg(debug_assertions)]
        if let Ok(id) = std::env::var("MELO_OPEN_ARTIST") {
            if let Ok(id) = id.parse::<u64>() {
                tasks.push(self.open_artist(id));
            }
        }

        // 开发辅助：登录后直接搜索关键词，便于检查真实数据布局。
        #[cfg(debug_assertions)]
        if let Ok(keyword) = std::env::var("MELO_SEARCH") {
            if !keyword.trim().is_empty() {
                self.search.input = keyword;
                tasks.push(self.submit_search());
            }
        }

        Task::batch(tasks)
    }

    fn reset_home_state(&mut self) {
        self.search = SearchState::default();
        self.library = LibraryState::default();
        self.playlist = None;
        self.liked_transfer = None;
        self.settings_page = false;
        self.netease_login_page = false;
        self.about_page = false;
        self.search_page = false;
        self.daily_page = false;
        self.fm_page = false;
        self.bili_page = false;
        self.artist = None;
        self.daily = DailyState::default();
        self.fm = FmState::default();
        // 本地收藏是设备级数据，切换账号或退出登录都不清空。
        self.covers.clear();
        self.circle_covers.clear();
        self.cover_disk.clear();
        self.cover_refreshed.clear();
        self.cover_fresh.clear();
    }

    fn submit_search(&mut self) -> Task<Message> {
        let keyword = self.search.input.trim().to_string();
        self.playlist = None;

        if keyword.is_empty() {
            self.search.query.clear();
            self.search.results.clear();
            self.search.total = 0;
            self.search.offset = 0;
            self.search.more_busy = false;
            self.search.submitted = false;
            self.search.error = None;
            return Task::none();
        }

        self.search.submitted = true;
        self.search.busy = true;
        self.search.more_busy = false;
        self.search.error = None;
        self.search.total = 0;
        self.search.offset = 0;
        self.search.query = keyword.clone();
        self.search.results.clear();
        // 提交搜索后直接进入独立的搜索页。
        self.search_page = true;
        self.settings_page = false;
        self.about_page = false;
        self.daily_page = false;
        self.fm_page = false;
        self.artist = None;

        if self.demo {
            self.search.busy = false;
            let songs = demo_songs();
            let covers = songs.iter().map(|song| song.cover(96)).collect();
            self.search.total = songs.len() as u64;
            self.search.offset = songs.len() as u32;
            self.search.results = songs;
            return self.ensure_covers(covers);
        }

        Task::perform(
            load_search(self.client.clone(), keyword, SEARCH_PAGE, 0),
            Message::SearchLoaded,
        )
    }

    /// 加载搜索结果的下一页：滚到列表底部时自动触发。
    fn search_more(&mut self) -> Task<Message> {
        let keyword = self.search.query.trim().to_string();
        if self.demo
            || self.search.more_busy
            || keyword.is_empty()
            || self.search.offset as u64 >= self.search.total
        {
            return Task::none();
        }

        self.search.more_busy = true;
        Task::perform(
            load_search(
                self.client.clone(),
                keyword,
                SEARCH_PAGE,
                self.search.offset,
            ),
            Message::SearchMoreLoaded,
        )
    }

    /// 歌单内搜索要覆盖「还没加载」的歌曲：网易云补齐 `track_ids` 里缺失的歌曲，
    /// 哔哩哔哩继续翻页，直到没有下一页。
    fn ensure_playlist_search_coverage(&mut self) -> Task<Message> {
        let Some(state) = &mut self.playlist else {
            return Task::none();
        };

        if state.bilibili {
            if state.bili_has_more && !state.bili_busy {
                state.bili_busy = true;
                let id = state.id;
                let page = state.bili_page + 1;
                return self.load_bili_folder(id, page);
            }
            return Task::none();
        }

        if state.local_liked || state.local_downloads || state.filling {
            return Task::none();
        }

        let Some(detail) = &state.detail else {
            return Task::none();
        };
        let missing = detail.missing_track_ids();
        if missing.is_empty() {
            return Task::none();
        }

        state.filling = true;
        let id = state.id;
        Task::perform(
            fill_playlist_tracks(self.client.clone(), missing),
            move |result| Message::PlaylistTracksFilled { id, result },
        )
    }

    /// 确保当前可见（经搜索过滤后的）歌曲封面已请求。
    /// 过滤后原来的行号会变，不能再用未过滤的窗口去猜。
    fn ensure_playlist_covers(&mut self, offset: f32) -> Task<Message> {
        let covers = {
            let Some(state) = &self.playlist else {
                return Task::none();
            };
            let Some(detail) = &state.detail else {
                return Task::none();
            };

            let filtered = views::home::filtered_track_indices(state, detail);
            let total = filtered.as_ref().map_or(detail.tracks.len(), Vec::len);
            let (first, last) = playlist_window(self.viewport.height, offset, total);
            (first..last)
                .map(|position| {
                    let index = filtered
                        .as_ref()
                        .map_or(position, |indices| indices[position]);
                    detail.tracks[index].cover(96)
                })
                .collect::<Vec<_>>()
        };

        self.ensure_covers(covers)
    }

    /// 拉取每日推荐；演示模式直接用示例歌曲。
    fn load_daily(&mut self) -> Task<Message> {
        self.daily.busy = true;
        self.daily.error = None;

        if self.demo {
            self.daily.busy = false;
            let songs = demo_songs();
            let covers = songs.iter().map(|song| song.cover(96)).collect();
            self.daily.songs = songs;
            return self.ensure_covers(covers);
        }

        Task::perform(load_daily_songs(self.client.clone()), Message::DailyLoaded)
    }

    /// 拉取私人 FM 推荐；演示模式直接用示例歌曲。
    fn load_fm(&mut self) -> Task<Message> {
        self.fm.busy = true;
        self.fm.error = None;

        if self.demo {
            self.fm.busy = false;
            let songs = demo_songs();
            let covers = songs.iter().map(|song| song.cover(96)).collect();
            self.fm.songs = songs;
            return self.ensure_covers(covers);
        }

        Task::perform(load_fm_songs(self.client.clone()), Message::FmLoaded)
    }

    /// 打开歌手页：资料与首屏歌曲一起取；已经进入播放页时回到首页展示。
    fn open_artist(&mut self, id: u64) -> Task<Message> {
        self.show_quality = false;
        self.show_queue = false;
        self.show_settings = false;
        self.artist = Some(ArtistState {
            id,
            detail: None,
            songs: Vec::new(),
            total: 0,
            offset: 0,
            busy: true,
            error: None,
            more_busy: false,
            bio_expanded: false,
            from_player: self.screen == Screen::NowPlaying,
        });
        self.view_fade = 1.0;
        if self.screen != Screen::Home {
            self.go(Screen::Home);
        }

        if self.demo {
            let detail = demo_artist(id);
            let songs = demo_songs();
            let covers = songs.iter().map(|song| song.cover(96)).collect();
            if let Some(state) = &mut self.artist {
                state.busy = false;
                state.total = songs.len() as u64;
                state.offset = songs.len() as u32;
                state.songs = songs;
                state.detail = Some(detail);
            }
            return self.ensure_covers(covers);
        }

        Task::perform(load_artist(self.client.clone(), id), move |result| {
            Message::ArtistLoaded { id, result }
        })
    }

    /// 歌手页滚到底部时自动加载更多。
    fn artist_more(&mut self) -> Task<Message> {
        let Some(state) = &self.artist else {
            return Task::none();
        };
        if state.busy || state.more_busy || state.offset as u64 >= state.total {
            return Task::none();
        }
        let id = state.id;
        let offset = state.offset;
        if let Some(state) = &mut self.artist {
            state.more_busy = true;
        }
        Task::perform(
            load_artist_songs(self.client.clone(), id, offset),
            move |result| Message::ArtistMoreLoaded { id, result },
        )
    }

    pub(crate) fn is_liked(&self, id: u64) -> bool {
        self.liked.contains(id)
    }

    /// 切换本地收藏：不请求网络，只更新内存、写盘并同步本地页。
    fn toggle_like(&mut self, song: Song) -> Task<Message> {
        if song.id == 0 {
            return Task::none();
        }

        let like = self.liked.toggle(song.clone());
        self.sync_local_liked_page(&song, like);

        match self.liked.save(&self.paths.liked_file()) {
            Ok(()) => self.show_toast(if like {
                "已收藏到本地「我喜欢的音乐」"
            } else {
                "已取消收藏"
            }),
            Err(error) => self.show_toast(format!("已切换红心，但本地保存失败：{error}")),
        }

        Task::none()
    }

    /// 执行导入 / 导出；失败时保留弹窗，把错误显示在里面。
    fn confirm_liked_transfer(&mut self) -> Task<Message> {
        let Some(transfer) = self.liked_transfer.take() else {
            return Task::none();
        };

        let path = expand_path(&transfer.path);
        if path.as_os_str().is_empty() {
            self.liked_transfer = Some(LikedTransfer {
                error: Some("请先填写文件路径。".to_string()),
                ..transfer
            });
            return Task::none();
        }

        if transfer.kind == LikedTransferKind::Export {
            return match self.liked.export_to(&path) {
                Ok(()) => {
                    self.show_toast(format!(
                        "已导出 {} 首收藏到 {}",
                        self.liked.len(),
                        shorten(&path.display().to_string(), 42)
                    ));
                    Task::none()
                }
                Err(error) => {
                    self.liked_transfer = Some(LikedTransfer {
                        error: Some(error),
                        ..transfer
                    });
                    Task::none()
                }
            };
        }

        match self.liked.import_from(&path) {
            Ok(report) => {
                let save_error = self.liked.save(&self.paths.liked_file()).err();
                let summary = match (report.added, report.skipped) {
                    (0, skipped) => format!("没有新增收藏（跳过 {skipped} 首）"),
                    (added, 0) => format!("已导入 {added} 首收藏"),
                    (added, skipped) => format!("已导入 {added} 首，跳过 {skipped} 首已收藏"),
                };
                self.show_toast(match save_error {
                    Some(error) => format!("{summary}，但本地保存失败：{error}"),
                    None => summary,
                });

                // 正在看本地收藏页时重建列表，新导入的歌会直接出现。
                let on_liked_page = self
                    .playlist
                    .as_ref()
                    .is_some_and(|state| state.local_liked);
                if on_liked_page {
                    self.open_liked()
                } else {
                    Task::none()
                }
            }
            Err(error) => {
                self.liked_transfer = Some(LikedTransfer {
                    error: Some(error),
                    ..transfer
                });
                Task::none()
            }
        }
    }

    /// 正在看本地收藏页时同步增删，不必重建整页。
    fn sync_local_liked_page(&mut self, song: &Song, like: bool) {
        let Some(state) = &mut self.playlist else {
            return;
        };
        if !state.local_liked {
            return;
        }
        if let Some(detail) = &mut state.detail {
            if like {
                detail.like_track(song.clone());
            } else {
                detail.unlike_track(song.id);
            }
            detail.track_count = detail.tracks.len() as u64;
            // 本地页不需要补齐逻辑，清掉 track_ids 以免出现“缺少歌曲”的提示。
            detail.track_ids.clear();
        }
    }

    /// 打开本地「我喜欢的音乐」页，不访问网络。
    fn open_liked(&mut self) -> Task<Message> {
        let detail = self.liked.as_playlist();
        let (first, last) = playlist_window(self.viewport.height, 0.0, detail.tracks.len());
        let mut covers: Vec<String> = Vec::with_capacity(last.saturating_sub(first));
        covers.extend(detail.tracks[first..last].iter().map(|song| song.cover(96)));

        self.playlist = Some(PlaylistState {
            id: 0,
            detail: Some(detail),
            busy: false,
            error: None,
            filling: false,
            local_liked: true,
            local_downloads: false,
            bilibili: false,
            bili_page: 0,
            bili_has_more: false,
            bili_busy: false,
            search: String::new(),
        });
        self.playlist_scroll = 0.0;
        self.settings_page = false;
        self.about_page = false;
        self.search_page = false;
        self.daily_page = false;
        self.fm_page = false;
        self.bili_page = false;
        self.artist = None;
        self.view_fade = 1.0;

        self.ensure_covers(covers)
    }

    // ======================================================================
    // 下载 / 本地音乐
    // ======================================================================

    pub(crate) fn is_downloaded(&self, id: u64) -> bool {
        self.download_info.contains_key(&id)
    }

    pub(crate) fn is_downloading(&self, id: u64) -> bool {
        self.download_inflight.contains(&id) || self.download_waiting.contains(&id)
    }

    /// 本地文件对应的（音质标签，大小），歌曲行里显示用。
    pub(crate) fn download_meta(&self, id: u64) -> Option<(&str, u64)> {
        self.download_info
            .get(&id)
            .map(|(label, size)| (label.as_str(), *size))
    }

    /// 重新从磁盘索引同步内存缓存（列表、占用、每首歌的大小）。
    fn refresh_downloads(&mut self) {
        let (tracks, usage) = {
            let store = self
                .downloads
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            (store.tracks(), store.usage())
        };

        self.download_info = tracks
            .iter()
            .map(|track| (track.song.id, (track.label.clone(), track.size)))
            .collect();
        self.download_tracks = tracks;
        self.downloads_usage = usage;
    }

    /// 把一首歌加入下载队列；同一首不会重复下载。
    fn download_song(&mut self, song: Song) -> Task<Message> {
        if song.id == 0 {
            return Task::none();
        }
        if self.is_downloaded(song.id) {
            self.show_toast("已在本地音乐中");
            return Task::none();
        }
        if self.is_downloading(song.id) {
            self.show_toast("正在下载这首歌曲");
            return Task::none();
        }

        let title = shorten(&song.name, 16);
        let quality = self.player.quality.label();
        self.download_waiting.insert(song.id);
        self.download_queue.push_back(song);
        self.show_toast(format!("开始下载「{title}」· {quality}"));
        self.pump_downloads()
    }

    /// 把队列里的歌曲交给异步任务，最多同时跑两首。
    fn pump_downloads(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        while self.download_inflight.len() < MAX_CONCURRENT_DOWNLOADS {
            let Some(song) = self.download_queue.pop_front() else {
                break;
            };
            self.download_waiting.remove(&song.id);
            if self.is_downloaded(song.id) {
                continue;
            }

            self.download_inflight.insert(song.id);
            let quality = self.player.quality;
            let client = self.client.clone();
            let bilibili = self.bili.clone();
            let store = Arc::clone(&self.downloads);
            let message_song = song.clone();
            tasks.push(Task::perform(
                download_track(client, bilibili, store, song, quality),
                move |result| Message::Downloaded {
                    song: message_song.clone(),
                    result,
                },
            ));
        }
        Task::batch(tasks)
    }

    /// 下载任务完成：文件已经由任务写入磁盘，这里只刷新界面。
    fn downloaded(&mut self, song: Song, result: Result<String, TrackError>) -> Task<Message> {
        self.download_inflight.remove(&song.id);
        let title = shorten(&song.name, 16);

        // 下载期间用户清空了本地音乐：把刚写回来的文件再删掉。
        if self.download_cancelled.remove(&song.id) {
            let mut store = self
                .downloads
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            store.remove(song.id);
            drop(store);
            self.refresh_downloads();
            self.sync_downloads_page();
            return self.pump_downloads();
        }

        match result {
            Ok(label) => {
                self.refresh_downloads();
                self.sync_downloads_page();
                self.show_toast(format!("已下载「{title}」· {label}"));
                let covers = self.ensure_covers(vec![song.cover(96), song.cover(420)]);
                Task::batch([covers, self.pump_downloads()])
            }
            Err(TrackError::Skipped(reason)) => {
                self.show_toast(format!("「{title}」无法下载：{}", shorten(&reason, 28)));
                self.pump_downloads()
            }
            Err(TrackError::Failed(error)) => {
                self.show_toast(format!("下载「{title}」失败：{}", shorten(&error, 28)));
                self.pump_downloads()
            }
        }
    }

    /// 从本地音乐删除一首（只删文件与索引，不影响当前播放与队列）。
    fn remove_download(&mut self, id: u64) -> Task<Message> {
        if !self.is_downloaded(id) {
            return Task::none();
        }
        let name = self
            .download_tracks
            .iter()
            .find(|track| track.song.id == id)
            .map(|track| shorten(&track.song.name, 16))
            .unwrap_or_default();

        {
            let mut store = self
                .downloads
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            store.remove(id);
        }

        // 万一这首歌还在排队，也一并取消。
        self.download_queue.retain(|song| song.id != id);
        self.download_waiting.remove(&id);
        if self.download_inflight.remove(&id) {
            self.download_cancelled.insert(id);
        }

        self.refresh_downloads();
        self.sync_downloads_page();
        self.show_toast(if name.is_empty() {
            "已从本地音乐移除".to_string()
        } else {
            format!("已从本地音乐移除「{name}」")
        });
        Task::none()
    }

    fn clear_downloads(&mut self) -> Task<Message> {
        let freed = self.downloads_usage;
        {
            let mut store = self
                .downloads
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            store.clear();
        }

        // 清空后，仍在下载的任务完成时不要再写回本地音乐。
        for id in self.download_inflight.drain() {
            self.download_cancelled.insert(id);
        }
        self.download_queue.clear();
        self.download_waiting.clear();

        self.refresh_downloads();
        self.sync_downloads_page();
        self.confirm_clear_downloads = false;
        self.show_toast(format!(
            "已清空本地音乐，释放 {}",
            audio_cache::format_bytes(freed)
        ));
        Task::none()
    }

    /// 打开本地音乐页：完全离线，只用本地索引构建列表。
    fn open_downloads(&mut self) -> Task<Message> {
        self.refresh_downloads();
        let detail = self.local_detail();
        let (first, last) = playlist_window(self.viewport.height, 0.0, detail.tracks.len());
        let mut covers: Vec<String> = detail.tracks[first..last]
            .iter()
            .map(|song| song.cover(96))
            .collect();
        if !detail.cover_img_url.is_empty() {
            covers.push(detail.cover(420));
        }

        self.playlist = Some(PlaylistState {
            id: 0,
            detail: Some(detail),
            busy: false,
            error: None,
            filling: false,
            local_liked: false,
            local_downloads: true,
            bilibili: false,
            bili_page: 0,
            bili_has_more: false,
            bili_busy: false,
            search: String::new(),
        });
        self.playlist_scroll = 0.0;
        self.settings_page = false;
        self.about_page = false;
        self.search_page = false;
        self.daily_page = false;
        self.fm_page = false;
        self.bili_page = false;
        self.artist = None;
        self.view_fade = 1.0;

        self.ensure_covers(covers)
    }

    /// 拉取哔哩哔哩收藏夹列表。
    fn load_bili_folders(&mut self) -> Task<Message> {
        if self.bili_state.folders_busy || !self.bili_session.is_logged_in() {
            return Task::none();
        }
        self.bili_state.folders_busy = true;
        self.bili_state.folders_error = None;
        Task::perform(
            fetch_bili_folders(self.bili.clone(), self.bili_session.mid),
            Message::BiliFoldersLoaded,
        )
    }

    /// 打开一个哔哩哔哩收藏夹（复用歌单页的列表与播放逻辑）。
    fn open_bili_folder(&mut self, id: u64) -> Task<Message> {
        self.playlist = Some(PlaylistState::bilibili(id));
        self.playlist_scroll = 0.0;
        self.settings_page = false;
        self.about_page = false;
        self.search_page = false;
        self.daily_page = false;
        self.fm_page = false;
        self.bili_page = true;
        self.artist = None;
        self.view_fade = 1.0;

        self.load_bili_folder(id, 1)
    }

    fn load_bili_folder(&self, id: u64, page: u32) -> Task<Message> {
        Task::perform(
            fetch_bili_folder(self.bili.clone(), id, page),
            move |result| Message::BiliFolderLoaded { id, page, result },
        )
    }

    /// 正在看本地音乐页时同步列表，不改变滚动位置。
    fn sync_downloads_page(&mut self) {
        if !self.on_local_page() {
            return;
        }
        let detail = self.local_detail();
        if let Some(state) = &mut self.playlist {
            state.detail = Some(detail);
        }
    }

    /// 本地音乐页 = 已下载歌曲 + 音乐文件夹里的歌曲。
    fn local_detail(&self) -> PlaylistDetail {
        let mut detail = download_store::detail(&self.download_tracks);
        detail
            .tracks
            .extend(self.local_tracks.iter().map(|file| file.song.clone()));
        detail.track_count = detail.tracks.len() as u64;
        if detail.cover_img_url.is_empty() {
            detail.cover_img_url = self
                .local_tracks
                .iter()
                .find_map(|file| {
                    file.song
                        .al
                        .as_ref()
                        .map(|album| album.pic_url.clone())
                        .filter(|url| !url.is_empty())
                })
                .unwrap_or_default();
        }
        detail.description = Some("下载与文件夹里的歌曲，断网也能播放。".to_string());
        detail
    }

    pub(crate) fn on_local_page(&self) -> bool {
        self.playlist
            .as_ref()
            .is_some_and(|state| state.local_downloads)
    }

    /// 本地文件夹歌曲的（格式，大小），歌曲行里显示用。
    pub(crate) fn local_meta(&self, id: u64) -> Option<(&str, u64)> {
        self.local_info
            .get(&id)
            .map(|(format, size)| (format.as_str(), *size))
    }

    /// 本地音乐页总共多少首（下载 + 文件夹）。
    pub(crate) fn local_song_count(&self) -> usize {
        self.download_tracks.len() + self.local_tracks.len()
    }

    /// 本地音乐总占用（下载 + 文件夹）。
    pub(crate) fn local_total_usage(&self) -> u64 {
        self.downloads_usage + self.local_usage
    }

    /// 添加一个音乐文件夹：校验路径、去重、持久化并开始扫描。
    fn add_local_folder(&mut self) -> Task<Message> {
        let Some(dialog) = self.local_folder_dialog.take() else {
            return Task::none();
        };

        let input = dialog.path.trim().to_string();
        let path = expand_path(&input);
        let error = if input.is_empty() {
            Some("请先填写文件夹路径。".to_string())
        } else if !path.is_dir() {
            Some("这个路径不是文件夹，或无法访问。".to_string())
        } else {
            None
        };
        if let Some(error) = error {
            self.local_folder_dialog = Some(LocalFolderDialog {
                path: dialog.path,
                error: Some(error),
            });
            return Task::none();
        }

        // 规范化路径，避免 `~/Music` 与 `/home/me/Music` 被当成两个文件夹。
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        if self.local_folders.iter().any(|folder| folder.path == path) {
            self.local_folder_dialog = Some(LocalFolderDialog {
                path: dialog.path,
                error: Some("这个文件夹已经在列表里了。".to_string()),
            });
            return Task::none();
        }

        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        self.local_folders.push(LocalFolderState {
            path,
            files: Vec::new(),
            error: None,
            scanning: true,
        });
        self.persist_local_folders();
        self.show_toast(format!("已添加文件夹「{}」，正在扫描…", shorten(&name, 22)));
        // 弹窗保持打开，方便连续添加多个目录；列表里会实时显示扫描进度。
        self.local_folder_dialog = Some(LocalFolderDialog {
            path: String::new(),
            error: None,
        });
        self.scan_local_folders()
    }

    /// 移除文件夹：只从列表里去掉，不碰磁盘上的文件。
    fn remove_local_folder(&mut self, index: usize) -> Task<Message> {
        if index >= self.local_folders.len() {
            return Task::none();
        }
        self.local_folders.remove(index);
        self.persist_local_folders();
        self.rebuild_local_index();
        self.sync_downloads_page();
        self.show_toast("已移除文件夹（不会删除磁盘文件）");

        // 正在扫描时马上重扫剩下的，旧结果会被序号丢弃。
        if self.local_scan_busy {
            self.scan_local_folders()
        } else {
            Task::none()
        }
    }

    /// 后台扫描所有已添加的文件夹。
    fn scan_local_folders(&mut self) -> Task<Message> {
        self.local_scan_seq = self.local_scan_seq.wrapping_add(1);
        let seq = self.local_scan_seq;
        let paths: Vec<PathBuf> = self
            .local_folders
            .iter()
            .map(|folder| folder.path.clone())
            .collect();
        if paths.is_empty() {
            self.local_scan_busy = false;
            return Task::none();
        }

        for folder in &mut self.local_folders {
            folder.scanning = true;
            folder.error = None;
        }
        self.local_scan_busy = true;
        Task::perform(local_files::scan_folders(paths), move |folders| {
            Message::LocalFoldersScanned { seq, folders }
        })
    }

    fn local_folders_scanned(&mut self, seq: u64, folders: Vec<FolderScan>) -> Task<Message> {
        if seq != self.local_scan_seq {
            return Task::none();
        }

        self.local_scan_busy = false;
        self.local_folders = folders
            .into_iter()
            .map(|scan| LocalFolderState {
                path: scan.path,
                files: scan.files,
                error: scan.error,
                scanning: false,
            })
            .collect();
        self.rebuild_local_index();
        self.sync_downloads_page();

        // 扫描完成后补一次封面：正在播放的本地歌、本地音乐页与本地收藏页
        // 都可能在扫描完成前就渲染出来了。
        let mut covers = Vec::new();
        if let Some(song) = self.player.current.as_ref() {
            covers.push(song.cover(600));
        }
        let on_liked = self
            .playlist
            .as_ref()
            .is_some_and(|state| state.local_liked);
        if self.on_local_page() || on_liked {
            if let Some(detail) = self
                .playlist
                .as_ref()
                .and_then(|state| state.detail.as_ref())
            {
                let (first, last) = playlist_window(
                    self.viewport.height,
                    self.playlist_scroll,
                    detail.tracks.len(),
                );
                covers.extend(detail.tracks[first..last].iter().map(|song| song.cover(96)));
                if !detail.cover_img_url.is_empty() {
                    covers.push(detail.cover(420));
                }
            }
        }
        self.ensure_covers(covers)
    }

    /// 重新汇总所有文件夹的歌曲（去重、统计占用、记录封面路径）。
    fn rebuild_local_index(&mut self) {
        self.local_tracks.clear();
        self.local_info.clear();
        self.local_cover_sources.clear();
        self.local_usage = 0;

        let files = local_files::dedupe(
            self.local_folders
                .iter()
                .flat_map(|folder| folder.files.iter().cloned())
                .collect(),
        );
        for file in files {
            self.local_info
                .insert(file.song.id, (file.format.clone(), file.size));
            self.local_usage = self.local_usage.saturating_add(file.size);

            let has_cover = file
                .song
                .al
                .as_ref()
                .is_some_and(|album| album.pic_url.starts_with(local_files::COVER_PREFIX));
            if has_cover {
                self.local_cover_sources
                    .insert(file.song.id, file.path.clone());
            }
            self.local_tracks.push(file);
        }
    }

    /// 文件夹列表写入 `music-folders.json`。
    fn persist_local_folders(&mut self) {
        let folders: Vec<PathBuf> = self
            .local_folders
            .iter()
            .map(|folder| folder.path.clone())
            .collect();
        if let Err(error) = local_files::save_folders(&self.paths.music_folders_file(), &folders) {
            self.storage_error = Some(error);
        }
    }

    /// `local-cover://{id}?param=...` → 对应的本地文件路径。
    fn local_cover_source(&self, url: &str) -> Option<PathBuf> {
        let rest = url.strip_prefix(local_files::COVER_PREFIX)?;
        let id = rest.split('?').next()?.parse::<u64>().ok()?;
        self.local_cover_sources.get(&id).cloned()
    }

    fn reload_library(&mut self) -> Task<Message> {
        if self.demo {
            self.library.playlists = demo_playlists();
            self.library.busy = false;
            let covers = self
                .library
                .playlists
                .iter()
                .map(|item| item.cover(300))
                .collect();
            return self.ensure_covers(covers);
        }

        let Some(user_id) = self.profile.as_ref().map(|profile| profile.user_id) else {
            return Task::none();
        };

        self.library.busy = true;
        self.library.error = None;

        Task::perform(
            load_playlists(self.client.clone(), user_id),
            Message::LibraryLoaded,
        )
    }

    /// 用上次的歌单快照撑住首屏：列表来自 `library.json`，封面直接读磁盘缓存。
    /// 没有快照或账号不匹配时什么也不做，等网络结果。
    fn restore_library_cache(&mut self) {
        if !self.library.playlists.is_empty() {
            return;
        }
        let Some(user_id) = self.profile.as_ref().map(|profile| profile.user_id) else {
            return;
        };
        let Some(snapshot) = LibrarySnapshot::load(&self.paths.library_file()) else {
            return;
        };
        if !snapshot.belongs_to(user_id) {
            return;
        }

        let urls = snapshot
            .playlists
            .iter()
            .map(|item| item.cover(300))
            .collect();
        self.library.playlists = snapshot.playlists;
        self.library.busy = true;
        self.library.error = None;
        self.prime_cached_covers(urls);
    }

    /// 把当前歌单列表写到快照文件；写失败不影响界面（它只是启动加速）。
    fn persist_library(&self) {
        if self.demo {
            return;
        }
        let Some(user_id) = self.profile.as_ref().map(|profile| profile.user_id) else {
            return;
        };
        let snapshot = LibrarySnapshot {
            user_id,
            playlists: self.library.playlists.clone(),
        };
        let _ = snapshot.save(&self.paths.library_file());
    }

    /// 同步把已经下载过的封面从磁盘读进内存，第一帧就能显示图片。
    ///
    /// 只在启动时用：异步的 `ensure_covers` 还会为每个 URL 刷新一次网络图。
    fn prime_cached_covers(&mut self, urls: Vec<String>) {
        let dir = self.paths.cover_cache_dir();
        let mut primed = 0;
        for url in urls {
            if primed >= SYNC_COVER_LIMIT {
                break;
            }
            if url.is_empty() || self.covers.contains_key(&url) {
                continue;
            }
            let Some(bytes) = cover_cache::read(&dir, &url) else {
                continue;
            };

            let cover = decorate_cover(&url, bytes);
            if let Some(circle) = cover.circle {
                self.circle_covers
                    .insert(url.clone(), image::Handle::from_bytes(circle));
            }
            self.covers
                .insert(url.clone(), image::Handle::from_bytes(cover.bytes));
            self.cover_disk.insert(url);
            primed += 1;
        }
    }

    /// 头像同样先从磁盘读出：侧边栏与歌单页不会先闪一个字母。
    fn prime_cached_avatar(&mut self) {
        if self.avatar.is_some() {
            return;
        }
        let Some(profile) = &self.profile else {
            return;
        };
        let url = profile.avatar_with_size(320);
        if url.is_empty() {
            return;
        }
        let Some(bytes) = cover_cache::read(&self.paths.cover_cache_dir(), &url) else {
            return;
        };
        if let Some(rounded) = avatar::round(&bytes, 320) {
            self.avatar = Some(image::Handle::from_bytes(rounded));
        }
    }

    fn open_playlist(&mut self, id: u64) -> Task<Message> {
        if self.profile.is_none() {
            return Task::none();
        }
        self.playlist = Some(PlaylistState {
            id,
            detail: None,
            busy: true,
            error: None,
            filling: false,
            local_liked: false,
            local_downloads: false,
            bilibili: false,
            bili_page: 0,
            bili_has_more: false,
            bili_busy: false,
            search: String::new(),
        });
        self.playlist_scroll = 0.0;
        self.settings_page = false;
        self.about_page = false;
        self.search_page = false;
        self.daily_page = false;
        self.fm_page = false;
        self.bili_page = false;
        self.artist = None;
        self.view_fade = 1.0;

        if self.demo {
            let detail = demo_detail(id);
            let cover = detail.cover(420);
            if let Some(state) = &mut self.playlist {
                state.detail = Some(detail);
                state.busy = false;
            }
            return self.ensure_covers(vec![cover]);
        }

        Task::perform(
            load_playlist(self.client.clone(), id),
            Message::PlaylistLoaded,
        )
    }

    /// 去重后批量准备封面：先读本地缓存（命中就立刻显示），再请求一次网络刷新。
    ///
    /// 每个 URL 每次启动最多查一次缓存、刷新一次，滚动或重绘不会重复请求。
    fn ensure_covers(&mut self, urls: Vec<String>) -> Task<Message> {
        let mut tasks = Vec::new();
        let dir = self.paths.cover_cache_dir();

        for url in urls {
            if url.is_empty() {
                continue;
            }

            // 本地文件夹里的内嵌封面：不发网络请求，先从缓存读，再从文件里取。
            if url.starts_with(local_files::COVER_PREFIX) {
                if let Some(path) = self.local_cover_source(&url) {
                    if !self.covers.contains_key(&url) && self.cover_disk.insert(url.clone()) {
                        let cached_url = url.clone();
                        tasks.push(Task::perform(
                            load_cached_cover(dir.clone(), url.clone()),
                            move |result| Message::CoverLoaded {
                                url: cached_url.clone(),
                                cached: true,
                                result,
                            },
                        ));
                    }
                    if self.cover_refreshed.insert(url.clone()) {
                        let fresh_url = url.clone();
                        tasks.push(Task::perform(
                            load_local_cover(dir.clone(), url.clone(), path),
                            move |result| Message::CoverLoaded {
                                url: fresh_url.clone(),
                                cached: false,
                                result,
                            },
                        ));
                    }
                }
                continue;
            }

            // 1. 本地缓存：命中就能马上显示，不用等网络。
            if !self.covers.contains_key(&url) && self.cover_disk.insert(url.clone()) {
                let cached_url = url.clone();
                tasks.push(Task::perform(
                    load_cached_cover(dir.clone(), url.clone()),
                    move |result| Message::CoverLoaded {
                        url: cached_url.clone(),
                        cached: true,
                        result,
                    },
                ));
            }

            // 2. 每次启动刷新一次：后台重新下载，成功后覆盖缓存与界面。
            if self.cover_refreshed.insert(url.clone()) {
                let fresh_url = url.clone();
                tasks.push(Task::perform(
                    load_cover(self.client.clone(), self.bili.clone(), dir.clone(), url),
                    move |result| Message::CoverLoaded {
                        url: fresh_url.clone(),
                        cached: false,
                        result,
                    },
                ));
            }
        }

        Task::batch(tasks)
    }

    /// 记录某区域刚刚滚动过：滚动条据此在 0.6 秒内保持可见。
    fn touch_scroll(&mut self, area: ScrollArea) {
        self.scroll_touched[area.index()] = Some(Instant::now());
    }

    /// 该区域滚动条当前的不透明度：0 表示完全隐藏。
    pub(crate) fn scroll_opacity(&self, area: ScrollArea) -> f32 {
        self.scroll_fade[area.index()]
    }

    /// 滚动条正在淡入 / 淡出：这段时间需要高帧率把它画顺。
    fn scroll_animating(&self) -> bool {
        let now = Instant::now();
        (0..self.scroll_fade.len()).any(|index| {
            let target = if scroll_recent(self.scroll_touched[index], now) {
                1.0
            } else {
                0.0
            };
            (self.scroll_fade[index] - target).abs() > 0.001
        })
    }

    /// 重新打开队列面板时恢复上次的滚动位置：
    /// 内容变短时先把位置拉回可见范围，再让滚动容器跳过去。
    fn restore_queue_view(&mut self) -> Task<Message> {
        let total = self.player.queue.len();
        let view_height = self.viewport.height.max(views::queue::ROW_HEIGHT);
        self.queue_scroll = views::queue::clamp_offset(total, self.queue_scroll, view_height);

        let restore = scrollable::scroll_to(
            views::queue::list_id(),
            scrollable::AbsoluteOffset {
                x: 0.0,
                y: self.queue_scroll,
            },
        );

        Task::batch([restore, self.ensure_queue_covers(self.queue_scroll)])
    }

    /// 只预取队列当前可见窗口附近的封面，避免超大队列一次性发起过多请求。
    fn ensure_queue_covers(&mut self, offset: f32) -> Task<Message> {
        let total = self.player.queue.len();
        if total == 0 {
            return Task::none();
        }

        let view_height = self.viewport.height.max(views::queue::ROW_HEIGHT);
        let (first, last, ..) = views::queue::visible_window(total, offset, view_height);
        let covers = self.player.queue[first..last]
            .iter()
            .map(|song| song.cover(96))
            .collect();
        self.ensure_covers(covers)
    }

    /// 用一组歌曲作为播放队列，从 `index` 开始。
    fn play_from(&mut self, songs: Vec<Song>, index: usize) -> Task<Message> {
        if songs.is_empty() || index >= songs.len() {
            return Task::none();
        }
        self.player.queue = songs;
        self.player.index = index;
        self.player.history.clear();
        // 普通队列不会自动续歌；FM 由 `play_fm_from` 在调用后再标回来。
        self.player.fm_queue = false;
        self.player.fm_fetching = false;
        self.player.auto_skips = 0;
        self.start_current()
    }

    /// 从 FM 列表开始播放：队列打上 FM 标记，播到队尾时自动续歌。
    fn play_fm_from(&mut self, songs: Vec<Song>, index: usize) -> Task<Message> {
        if songs.is_empty() || index >= songs.len() {
            return Task::none();
        }
        let task = self.play_from(songs, index);
        self.player.fm_queue = true;
        task
    }

    /// 把一首歌插到当前播放项后面（「添加到下一首」）；当前没有歌曲时直接开始播放。
    ///
    /// 连续添加时最后一次添加的排在最前面：A 正在播放，先加 H、再加 K，
    /// 队列会变成 A K H B C。
    fn enqueue_next(&mut self, song: Song) -> Task<Message> {
        if self.player.current.is_none() {
            self.player.queue.push(song);
            self.player.index = self.player.queue.len() - 1;
            return self.start_current();
        }

        let cover = song.cover(96);
        let title = shorten(&song.name, 16);
        insert_after_current(
            &mut self.player.queue,
            &mut self.player.history,
            self.player.index,
            song,
        );
        self.mark_player_dirty();
        self.show_toast(if title.is_empty() {
            "已添加到下一首".to_string()
        } else {
            format!("已添加到下一首「{title}」")
        });
        self.ensure_covers(vec![cover])
    }

    /// 清空当前播放状态，但保留队列本身由调用方处理。
    fn stop_and_clear(&mut self) {
        self.player.engine.stop();
        self.player.current = None;
        self.player.loading = false;
        self.player.error = None;
        self.player.position = Duration::ZERO;
        self.player.duration = Duration::ZERO;
        self.player.seek_pending = None;
        self.player.seek_commit = None;
        self.player.pending_resume = None;
        self.player.pause_after_load = false;
        self.player.loaded_quality.clear();
        self.player.fm_queue = false;
        self.player.fm_fetching = false;
        self.player.auto_skips = 0;
        self.player.spectrum = vec![0.0; BAND_COUNT];
        self.player.processor.reset(Some(BAND_COUNT));
        self.ring.reset();
        self.mark_player_dirty();

        if self.screen == Screen::NowPlaying {
            self.go(Screen::Home);
        }
    }

    /// 换音质后重新请求当前曲目：保留进度，暂停状态也保持不变。
    fn reload_after_quality_change(&mut self) -> Task<Message> {
        if self.demo || self.player.current.is_none() {
            return Task::none();
        }

        let position = self.player.position;
        // 正在加载的曲目按“用户想播”处理，加载完后直接播放。
        let paused = !self.player.is_playing() && !self.player.loading;
        let task = self.begin_current(position);
        // `begin_current` 会重置标志，所以要在它之后再记下暂停状态。
        self.player.pause_after_load = paused;
        task
    }

    fn start_current(&mut self) -> Task<Message> {
        // 手动切歌 / 从列表播放：丢掉上次退出时留下的恢复进度。
        self.player.pending_resume = None;
        self.begin_current(Duration::ZERO)
    }

    /// 从队列当前项开始播放；`resume` 会在音频加载完成后跳转到该位置。
    fn begin_current(&mut self, resume: Duration) -> Task<Message> {
        let Some(song) = self.player.queue.get(self.player.index).cloned() else {
            return Task::none();
        };

        self.player.load_seq = self.player.load_seq.wrapping_add(1);
        self.player.current = Some(song.clone());
        self.player.engine.stop();
        self.player.pause_after_load = false;
        self.player.error = None;
        self.player.position = resume;
        self.player.pending_resume = (!resume.is_zero()).then_some(resume);
        self.player.seek_pending = None;
        self.player.seek_commit = None;
        self.player.duration = Duration::from_millis(song.dt);
        self.player.loaded_quality.clear();
        self.player.processor.reset(Some(BAND_COUNT));
        self.player.spectrum = vec![0.0; BAND_COUNT];
        self.ring.reset();
        self.mark_player_dirty();

        let cover = self.ensure_covers(vec![song.cover(600)]);

        // 演示数据没有真实音源，只展示界面。
        if self.demo {
            self.player.loading = false;
            self.player.pending_resume = None;
            return cover;
        }

        self.player.loading = true;
        let id = song.id;
        let quality = self.player.quality;
        let seq = self.player.load_seq;
        let cache = Arc::clone(&self.cache);
        let downloads = Arc::clone(&self.downloads);
        let local = (!song.local_path.is_empty()).then(|| PathBuf::from(&song.local_path));
        Task::batch([
            cover,
            Task::perform(
                load_track(
                    self.client.clone(),
                    self.bili.clone(),
                    cache,
                    downloads,
                    song,
                    quality,
                    local,
                ),
                move |result| Message::TrackLoaded { id, seq, result },
            ),
        ])
    }

    /// 自动续播：单曲循环优先于随机，唱完就重播当前曲。
    /// 私人 FM 队列走到队尾时自动拉下一批。
    fn next_track(&mut self) -> Task<Message> {
        match next_index(self.player.mode, self.player.queue.len(), self.player.index) {
            Some(next) => self.switch_to(next),
            None => self.refill_fm(),
        }
    }

    /// 手动「下一首」：切歌优先于循环规则，单曲循环不会拦住手动切歌；
    /// 随机开着就走随机。
    fn skip_next(&mut self) -> Task<Message> {
        self.player.auto_skips = 0;
        match skip_index(self.player.mode, self.player.queue.len(), self.player.index) {
            Some(next) => self.switch_to(next),
            None => self.refill_fm(),
        }
    }

    /// 私人 FM 播到队尾：拉一批新歌接在后面，并直接播第一首新的。
    fn refill_fm(&mut self) -> Task<Message> {
        if !self.player.fm_queue || self.player.fm_fetching {
            return Task::none();
        }
        self.player.fm_fetching = true;

        if self.demo {
            // 演示模式没有真实推荐：从头循环示例歌曲，方便观察自动续播。
            self.player.fm_fetching = false;
            return self.switch_to(0);
        }

        // 续歌期间进度条显示加载光效，同时挡住「已完成」重复触发自动下一首。
        self.player.loading = true;
        Task::perform(load_fm_songs(self.client.clone()), Message::FmMoreLoaded)
    }

    /// VIP / 购买 / 无版权歌曲：不报错，直接切到下一首并弹一个悬浮提示。
    fn auto_skip(&mut self, reason: String) -> Task<Message> {
        let next = skip_index(self.player.mode, self.player.queue.len(), self.player.index);
        let fm_refill = next.is_none() && self.player.fm_queue && !self.player.fm_fetching;

        // 队列已经到头：没有可跳的下一首，只能把原因告诉用户。
        if next.is_none() && !fm_refill {
            self.player.auto_skips = 0;
            self.player.error = Some(format!("{reason}队列里也没有下一首可播放的歌曲。"));
            return Task::none();
        }

        // 整张队列都不可播时不要一直空跑网络。
        if self.player.auto_skips >= MAX_AUTO_SKIP {
            self.player.auto_skips = 0;
            self.player.error = Some("连续多首歌曲都无法播放，已停止自动跳过。".to_string());
            return Task::none();
        }

        self.player.auto_skips += 1;
        self.player.error = None;

        let title = self
            .player
            .current
            .as_ref()
            .map(|song| shorten(&song.name, 16))
            .unwrap_or_default();
        self.show_toast(if title.is_empty() {
            "已自动跳过".to_string()
        } else {
            format!("已自动跳过「{title}」")
        });

        match next {
            Some(next) => self.switch_to(next),
            None => self.refill_fm(),
        }
    }

    /// 顶部 / 底部悬浮提示；新的提示会直接替换旧的。
    fn show_toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast {
            text: text.into(),
            shown_at: Instant::now(),
        });
    }

    /// 切到队列里的某一首；真的换了位置时，随机模式把旧曲记进历史供「上一首」回退。
    fn switch_to(&mut self, next: usize) -> Task<Message> {
        if next != self.player.index {
            if self.player.mode.shuffle {
                push_history(&mut self.player.history, self.player.index);
            }
            self.player.index = next;
        }
        self.start_current()
    }

    /// 播放超过 3 秒时，上一首先回到开头（和主流播放器一致）。
    fn previous_track(&mut self) -> Task<Message> {
        self.player.auto_skips = 0;
        if self.player.position > Duration::from_secs(3) {
            return self.restart_current();
        }

        // 随机模式没有固定的“上一首”，按播放历史回退。
        if self.player.mode.shuffle {
            if let Some(prev) = pop_history(
                &mut self.player.history,
                self.player.queue.len(),
                self.player.index,
            ) {
                self.player.index = prev;
                return self.start_current();
            }
            return self.restart_current();
        }

        if self.player.index > 0 {
            self.player.index -= 1;
            return self.start_current();
        }

        // 从头再往前：列表循环 / 单曲循环都算“还在循环”，手动切歌应该回到队尾。
        if matches!(self.player.mode.repeat, RepeatMode::All | RepeatMode::One)
            && self.player.queue.len() > 1
        {
            self.player.index = self.player.queue.len() - 1;
            return self.start_current();
        }
        self.restart_current()
    }

    /// 回到当前曲目的开头。
    fn restart_current(&mut self) -> Task<Message> {
        self.player.position = Duration::ZERO;
        self.player.seek_pending = Some((Duration::ZERO, Instant::now()));
        self.player.seek_commit = None;
        self.player.engine.seek(Duration::ZERO);
        self.mark_player_dirty();
        Task::none()
    }

    /// 指针停稳后，把拖动产生的目标位置真正交给音频线程。
    ///
    /// 拖动时 `SeekTo` 会随鼠标事件高频触发；只有停顿超过
    /// `SEEK_COMMIT_DELAY` 才提交一次，避免音频线程被连续的
    /// `try_seek` 堆满（听感上是“倒带”声）。
    fn flush_seek(&mut self) {
        let Some((target, requested)) = self.player.seek_commit else {
            return;
        };
        if requested.elapsed() < SEEK_COMMIT_DELAY {
            return;
        }

        self.player.seek_commit = None;
        // 音频还没加载完成：恢复点已经更新，等 TrackLoaded 时再 seek。
        if self.player.loading {
            return;
        }
        self.player.engine.seek(target);
    }

    /// 每次界面 Tick 都同步一次播放器快照与频谱。
    fn refresh_player(&mut self) -> Task<Message> {
        let snapshot = self.player.engine.snapshot();

        // 曲目元数据（song.dt）比解码器的时长估计可靠：MP3 的 total_duration
        // 常把编码器延迟算进去（实测会多出几秒），用它做分母会让进度条偏慢。
        let metadata_duration = self
            .player
            .current
            .as_ref()
            .map(|song| Duration::from_millis(song.dt))
            .filter(|duration| !duration.is_zero());
        self.player.duration = match (metadata_duration, snapshot.duration) {
            (Some(metadata), _) => metadata,
            (None, decoded) if !decoded.is_zero() => decoded,
            (None, _) => self.player.duration,
        };
        self.player.position = if self.player.duration.is_zero() {
            snapshot.position
        } else {
            snapshot.position.min(self.player.duration)
        };

        // 恢复后音频还没加载：进度条先停在退出时的位置，等 TrackLoaded 再真正 seek。
        if let Some(position) = self.player.pending_resume {
            self.player.position = position;
        }

        // 拖动进度条时先用目标位置占位，避免音频线程处理 seek 之前
        // 被旧快照把滑块拉回去。追上（或超时）之后恢复跟随真实进度。
        if let Some((target, started)) = self.player.seek_pending {
            let caught_up = {
                let diff = if snapshot.position > target {
                    snapshot.position - target
                } else {
                    target - snapshot.position
                };
                diff <= Duration::from_millis(400)
            };
            if caught_up || started.elapsed() > Duration::from_millis(1_500) {
                self.player.seek_pending = None;
            } else {
                self.player.position = target;
            }
        }
        self.player.processor.set_gains(self.visual.gains());

        let playing = matches!(snapshot.state, nm_audio::State::Playing);
        if playing {
            let PlayerState {
                engine,
                analyzer,
                processor,
                spectrum,
                samples,
                ..
            } = &mut self.player;
            let (channels, sample_rate) = engine.samples_into(samples);
            if !samples.is_empty() {
                analyzer.set_sample_rate(sample_rate as usize);
                let bands = analyzer.bands(samples, channels);
                processor.process_into(bands, spectrum);
            }
        } else if self.player.spectrum.iter().any(|value| *value > 0.001) {
            // 暂停 / 停止时让频谱自然落下，而不是瞬间归零。
            const SILENCE: [f64; BAND_COUNT] = [0.0; BAND_COUNT];
            self.player
                .processor
                .process_into(&SILENCE, &mut self.player.spectrum);
        }

        // 演示模式没有音源：喂一段缓慢变化的假频谱，方便预览可视化效果。
        #[cfg(debug_assertions)]
        if self.demo {
            let t = self.phase as f64;
            self.player
                .spectrum
                .iter_mut()
                .enumerate()
                .for_each(|(i, value)| {
                    let pos = i as f64 / BAND_COUNT as f64;
                    let wave = (t * 1.7 + pos * 11.0).sin() * 0.5 + 0.5;
                    let beat = (t * 3.1).sin().abs();
                    *value = wave * (1.0 - pos * 0.45) * (0.55 + 0.45 * beat);
                });
        }

        match snapshot.state {
            nm_audio::State::Failed if self.player.current.is_some() => {
                self.player.loading = false;
                if self.player.error.is_none() {
                    self.player.error = self.player.engine.error();
                }
            }
            nm_audio::State::Finished if !self.player.loading => {
                return self.next_track();
            }
            _ => {}
        }

        Task::none()
    }

    /// 退出网易云账号：清掉会话与播放状态，在首页内容区打开登录面板。
    fn sign_out(&mut self) -> Task<Message> {
        self.client.clear_cookies();
        self.session.clear();
        self.persist();

        self.player.engine.stop();
        self.player.queue.clear();
        self.player.index = 0;
        self.player.current = None;
        self.player.loading = false;
        self.player.error = None;
        self.player.seek_pending = None;
        self.player.seek_commit = None;
        self.player.pending_resume = None;
        self.player.loaded_quality.clear();
        self.player.fm_queue = false;
        self.player.fm_fetching = false;
        self.player.auto_skips = 0;
        self.player.spectrum = vec![0.0; BAND_COUNT];
        self.player.processor.reset(Some(BAND_COUNT));
        self.ring.reset();

        self.profile = None;
        self.artist = None;
        self.avatar = None;
        self.avatar_busy = false;
        self.show_settings = false;
        self.show_quality = false;
        self.show_queue = false;
        self.login_error = None;
        self.restore_error = None;
        self.cookie_input.clear();
        self.cookie_busy = false;
        self.reset_home_state();
        // 哔哩哔哩仍有效时直接留在收藏夹；否则在内容区打开登录面板。
        if self.bili_session.is_logged_in() {
            self.bili_page = true;
            self.view_fade = 1.0;
        } else {
            self.open_netease_login();
        }

        // 退出登录后不再恢复上一个账号的播放队列。
        self.persist_player();

        Task::none()
    }

    /// 在首页内容区打开网易云登录面板（不再切到独立的登录屏）。
    fn open_netease_login(&mut self) {
        self.netease_login_page = true;
        self.login_error = None;
        self.settings_page = false;
        self.about_page = false;
        self.search_page = false;
        self.daily_page = false;
        self.fm_page = false;
        self.bili_page = false;
        self.playlist = None;
        self.artist = None;
        self.view_fade = 1.0;
        if self.screen != Screen::NowPlaying {
            self.go(Screen::Home);
        }
    }

    fn persist(&mut self) {
        match self.session.save(&self.paths.session_file()) {
            Ok(()) => self.storage_error = None,
            Err(error) => self.storage_error = Some(error),
        }
    }

    fn persist_visual(&mut self) {
        if let Err(error) = self.visual.save(&self.paths.visualizer_file()) {
            self.storage_error = Some(error);
        }
    }

    /// 切换页面并触发淡入过渡；同一页面重复设置不会重放动画。
    fn go(&mut self, screen: Screen) {
        if self.screen == screen {
            return;
        }
        self.screen = screen;
        self.screen_fade = 1.0;
    }

    /// 播放状态有变化：等下一次 Tick 统一落盘，避免拖动进度条时高频写文件。
    fn mark_player_dirty(&mut self) {
        self.player_dirty = true;
    }

    /// 立即把待写入的界面偏好落盘。
    fn flush_ui_settings(&mut self) {
        if self.ui_save_at.take().is_none() {
            return;
        }
        if let Err(error) = self.ui.save(&self.paths.settings_file()) {
            self.storage_error = Some(error);
        }
    }

    /// 把队列、当前项、模式、音量、音质与进度写进 `player.json`。
    fn persist_player(&mut self) {
        let snapshot = PlayerSnapshot {
            queue: self.player.queue.clone(),
            index: self.player.index,
            mode: self.player.mode,
            volume: self.player.volume,
            quality: self.player.quality,
            position: self.player.position.as_secs_f64(),
        };
        if let Err(error) = snapshot.save(&self.paths.player_file()) {
            self.storage_error = Some(error);
        }
        self.player_dirty = false;
        self.last_player_save = Instant::now();
    }

    /// 有变化时最多每秒落盘一次；播放中每 5 秒刷新一次进度。
    fn maybe_persist_player(&mut self) {
        let interval = if self.player_dirty {
            Duration::from_secs(1)
        } else if self.player.is_playing() {
            Duration::from_secs(5)
        } else {
            return;
        };

        if self.last_player_save.elapsed() >= interval {
            self.persist_player();
        }
    }

    fn view(&self) -> Element<'_> {
        let body: Element<'_> = match self.screen {
            Screen::Restoring => views::restoring::view(self),
            Screen::Home => views::home::view(self),
            Screen::NowPlaying => views::player::page(self),
        };

        // 以下是固定层数的浮层：装饰背景、页面、淡入、窗口按钮、队列、设置、
        // 音质、收藏导入导出、提示。
        //
        // 每一层都必须「始终存在」，隐藏时用空层占位。iced 在组件树某一层的类型
        // 变化时会整棵重建子树，滚动容器的偏移等内部状态也会跟着清零——所以不能
        // 用 `if 打开 { stack![body, 浮层] } else { body }` 这样的按需包裹，
        // 否则点个红心弹出一条提示就会让列表瞬间跳回顶部。

        // 装饰背景：浅色页面铺天空渐变、柔光与弧线；播放页有自己的封面氛围底。
        let backdrop_layer: Element<'_> = match self.screen {
            Screen::NowPlaying => blank_layer(),
            _ => views::backdrop::layer(),
        };

        // 页面切换的淡入：只改透明度，层本身一直在。
        let fade = self.screen_fade.clamp(0.0, 1.0);
        let dark = self.screen == Screen::NowPlaying;
        let fade_layer: Element<'_> = iced::widget::container(iced::widget::row![])
            .width(iced::Length::Fill)
            .height(iced::Length::Fill)
            .style(move |_| {
                if dark {
                    theme::dark_fade(fade)
                } else {
                    theme::page_fade(fade)
                }
            })
            .into();

        // 顶栏已移除：页面直接铺满窗口，最小化 / 关闭悬浮在右上角。
        // 播放页把可视化设置齿轮也收进同一枚胶囊里，避免右上角散着几个图标。
        // 它放在浮层下面，队列 / 设置打开时会被遮住，不会压住面板头部。
        let player_page = self.screen == Screen::NowPlaying && self.player.current.is_some();
        let window_layer = widgets::window_controls(player_page, player_page);

        // 队列是全局浮层：首页和正在播放页都能打开。
        let queue_layer: Element<'_> = if self.show_queue {
            views::queue::panel(self)
        } else {
            blank_layer()
        };

        // 设置也是全局浮层；它盖在队列之上，两者不会同时打开。
        let settings_layer: Element<'_> = if self.show_settings {
            views::player::settings_panel(self)
        } else {
            blank_layer()
        };

        // 音质选择弹窗居中显示，打开时其它浮层已经收起。
        let quality_layer: Element<'_> = if self.show_quality {
            views::player::quality_dialog(self)
        } else {
            blank_layer()
        };

        // 本地收藏的导入 / 导出弹窗（从「我喜欢的音乐」页打开）。
        let transfer_layer: Element<'_> = if self.liked_transfer.is_some() {
            views::home::liked_transfer_dialog(self)
        } else {
            blank_layer()
        };

        // 「清空本地音乐」确认弹窗。
        let clear_layer: Element<'_> = if self.confirm_clear_downloads {
            views::home::clear_downloads_dialog(self)
        } else {
            blank_layer()
        };

        // 添加 / 管理本地音乐文件夹。
        let folders_layer: Element<'_> = if self.local_folder_dialog.is_some() {
            views::home::local_folders_dialog(self)
        } else {
            blank_layer()
        };

        // 悬浮提示压在最上层：底部播放条 / 播放页控制台上方居中，自动跳过时可见。
        let toast_layer: Element<'_> = match self
            .toast
            .as_ref()
            .map(|toast| (toast, toast_opacity(toast.shown_at.elapsed())))
        {
            Some((toast, opacity)) if opacity > 0.001 => {
                let bottom = match self.screen {
                    Screen::NowPlaying => 96.0,
                    _ if self.player.current.is_some() => views::player::BAR_HEIGHT + 16.0,
                    _ => 28.0,
                };
                iced::widget::container(widgets::slide_down(
                    widgets::toast(&toast.text, opacity),
                    (1.0 - opacity) * 8.0,
                ))
                .width(iced::Length::Fill)
                .height(iced::Length::Fill)
                .align_x(iced::Alignment::Center)
                .align_y(iced::Alignment::End)
                .padding(iced::Padding {
                    top: 0.0,
                    right: 0.0,
                    bottom,
                    left: 0.0,
                })
                .into()
            }
            _ => blank_layer(),
        };

        // 首次启动的使用须知：压在所有页面与浮层之上，确认前拦截全部操作。
        let disclaimer_layer: Element<'_> = if self.ui.disclaimer_accepted {
            blank_layer()
        } else {
            views::disclaimer::view()
        };

        // 平铺风格：不再铺柔光装饰，整页只用单色背景与卡片。
        iced::widget::container(
            iced::widget::Stack::with_children(vec![
                backdrop_layer,
                body,
                fade_layer,
                window_layer,
                queue_layer,
                settings_layer,
                quality_layer,
                transfer_layer,
                clear_layer,
                folders_layer,
                toast_layer,
                disclaimer_layer,
            ])
            .width(iced::Length::Fill)
            .height(iced::Length::Fill),
        )
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .style(theme::page)
        .into()
    }
}

/// 永远存在的空浮层：让组件树保持固定形状，不因为浮层开关而重建页面子树。
fn blank_layer<'a>() -> Element<'a> {
    iced::widget::container(iced::widget::row![])
        .width(iced::Length::Fill)
        .height(iced::Length::Fill)
        .into()
}

/// 把 `current` 以 `step` 为上限向 `target` 逼近，用于平滑淡入淡出。
fn approach(current: f32, target: f32, step: f32) -> f32 {
    let delta = target - current;
    if delta.abs() <= step {
        target
    } else {
        current + step.copysign(delta)
    }
}

/// 悬浮提示的不透明度：短暂淡入、停留、然后淡出。
fn toast_opacity(elapsed: Duration) -> f32 {
    let life = TOAST_LIFE.as_secs_f32();
    let t = elapsed.as_secs_f32();
    if t >= life {
        0.0
    } else if t < TOAST_FADE_IN {
        (t / TOAST_FADE_IN).clamp(0.0, 1.0)
    } else if t > life - TOAST_FADE_OUT {
        ((life - t) / TOAST_FADE_OUT).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// 截断过长歌名，避免悬浮提示被撑得过宽。
fn shorten(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let mut output: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        output.push('…');
    }
    output
}

/// 把用户输入的路径展开成可以直接打开的形式：支持 `~/` 前缀与首尾空格。
fn expand_path(input: &str) -> PathBuf {
    let trimmed = input.trim();
    match trimmed.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .unwrap_or_else(|| PathBuf::from(trimmed)),
        None => PathBuf::from(trimmed),
    }
}

/// 播放页的指针移动：悬停区判定由 Tick 完成。
fn dock_pointer_event(
    event: iced::Event,
    _status: iced::event::Status,
    _window: window::Id,
) -> Option<Message> {
    match event {
        iced::Event::Mouse(mouse::Event::CursorMoved { position }) => {
            Some(Message::PointerMoved(position))
        }
        // 指针离开窗口：挪到窗口外，控制台随之下滑收起。
        iced::Event::Mouse(mouse::Event::CursorLeft) => {
            Some(Message::PointerMoved(Point::new(-1.0, -1.0)))
        }
        _ => None,
    }
}

/// 从队列里移除一项后，播放器需要做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueueChange {
    /// 当前歌曲不受影响。
    Keep,
    /// 当前歌曲被移除，需要播放新位置上的歌。
    Play,
    /// 队列空了，需要停止播放。
    Stop,
}

fn remove_queue_entry(queue: &mut Vec<Song>, index: &mut usize, remove: usize) -> QueueChange {
    if remove >= queue.len() {
        return QueueChange::Keep;
    }

    let was_current = remove == *index;
    queue.remove(remove);

    if queue.is_empty() {
        *index = 0;
        return QueueChange::Stop;
    }

    if was_current {
        *index = (*index).min(queue.len() - 1);
        QueueChange::Play
    } else {
        if remove < *index {
            *index -= 1;
        }
        QueueChange::Keep
    }
}

/// 根据播放模式计算下一首的索引；`None` 表示顺序模式下已到队尾、应停止。
/// 从快照里取出可恢复的 `(下标, 时长, 进度)`；空队列返回 `None`。
fn restored_track(snapshot: &PlayerSnapshot) -> Option<(usize, Duration, Duration)> {
    if snapshot.is_empty() {
        return None;
    }

    let index = snapshot.index.min(snapshot.queue.len() - 1);
    let duration = Duration::from_millis(snapshot.queue[index].dt);
    Some((index, duration, snapshot.position(duration)))
}

fn next_index(mode: PlayMode, len: usize, index: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    // 单曲循环优先于随机：明确要重复当前曲时就不抽下一首。
    if mode.repeat == RepeatMode::One {
        return Some(index);
    }
    if mode.shuffle {
        return shuffle_index(len, index);
    }
    match mode.repeat {
        RepeatMode::Off => (index + 1 < len).then_some(index + 1),
        RepeatMode::All => Some((index + 1) % len),
        RepeatMode::One => Some(index),
    }
}

/// 手动「下一首」：切歌优先于循环规则。
///
/// - 随机开着就随机，单曲循环不再拦手动切歌；
/// - 顺序走到队尾时，只要还开着循环（列表 / 单曲）就回到队首。
fn skip_index(mode: PlayMode, len: usize, index: usize) -> Option<usize> {
    if len == 0 {
        return None;
    }
    if mode.shuffle {
        return shuffle_index(len, index);
    }
    if index + 1 < len {
        return Some(index + 1);
    }
    match mode.repeat {
        RepeatMode::Off => None,
        RepeatMode::All | RepeatMode::One => Some(0),
    }
}

/// 随机模式下的下一首：等概率随机，命中当前曲时向后顺延一位。
fn shuffle_index(len: usize, index: usize) -> Option<usize> {
    match len {
        0 => None,
        1 => Some(0),
        _ => {
            let mut next = rand::thread_rng().gen_range(0..len);
            if next == index {
                next = (next + 1) % len;
            }
            Some(next)
        }
    }
}

/// 把刚播过的索引压入随机历史；连续重复的索引只记一次。
fn push_history(history: &mut Vec<usize>, index: usize) {
    if history.last() == Some(&index) {
        return;
    }
    if history.len() >= MAX_HISTORY {
        history.remove(0);
    }
    history.push(index);
}

/// 随机模式下回退一首：跳过已不在队列里的历史索引与当前曲。
fn pop_history(history: &mut Vec<usize>, len: usize, current: usize) -> Option<usize> {
    while let Some(prev) = history.pop() {
        if prev < len && prev != current {
            return Some(prev);
        }
    }
    None
}

/// 队列里移除一项后同步历史索引，保持后续行号不错位。
fn drop_history_entry(history: &mut Vec<usize>, removed: usize) {
    history.retain(|&index| index != removed);
    for index in history.iter_mut() {
        if *index > removed {
            *index -= 1;
        }
    }
}

/// 把歌曲插到当前播放项之后（「添加到下一首」），并把随机播放历史里
/// 指向插入点及其之后的索引整体后移一位，避免回退时跳到别的歌上。
/// 返回插入后的下标。
fn insert_after_current(
    queue: &mut Vec<Song>,
    history: &mut [usize],
    current: usize,
    song: Song,
) -> usize {
    let position = (current + 1).min(queue.len());
    queue.insert(position, song);
    for index in history.iter_mut() {
        if *index >= position {
            *index += 1;
        }
    }
    position
}

/// 把一批新歌曲接到列表尾部，跳过已经存在的歌（FM 续歌 / 歌手页翻页共用）。
/// 返回新歌第一首的下标；一首新歌都没有时返回 `None`。
fn append_unique(queue: &mut Vec<Song>, songs: Vec<Song>) -> Option<usize> {
    let mut seen: HashSet<u64> = queue.iter().map(|song| song.id).collect();
    let start = queue.len();
    queue.extend(songs.into_iter().filter(|song| seen.insert(song.id)));
    (queue.len() > start).then_some(start)
}

fn volume_after_scroll(volume: f32, delta: mouse::ScrollDelta) -> f32 {
    let step = match delta {
        mouse::ScrollDelta::Lines { y, .. } => y * 0.05,
        mouse::ScrollDelta::Pixels { y, .. } => y * 0.002,
    };
    (volume + step).clamp(0.0, 1.0)
}

/// 点击音量图标：在静音与上一次音量之间切换。
///
/// 当前音量本来就是 0（且不是静音）时，恢复到默认音量，避免“点不开”。
fn toggle_mute(volume: &mut f32, muted_volume: &mut Option<f32>) {
    match muted_volume.take() {
        Some(previous) => *volume = previous,
        None if *volume > 0.0 => {
            *muted_volume = Some(*volume);
            *volume = 0.0;
        }
        None => *volume = DEFAULT_VOLUME,
    }
    *volume = volume.clamp(0.0, 1.0);
}

async fn load_account(client: NeteaseClient) -> Result<Option<Profile>, String> {
    client.account().await.map_err(|error| error.to_string())
}

/// 取播放地址并下载整首音频；优先用用户下载的本地音乐，其次命中播放缓存，
/// 都没有才走网络。
async fn load_track(
    client: NeteaseClient,
    bilibili: BilibiliClient,
    cache: Arc<Mutex<AudioCache>>,
    downloads: Arc<Mutex<DownloadStore>>,
    song: Song,
    quality: AudioQuality,
    local: Option<PathBuf>,
) -> Result<LoadedTrack, TrackError> {
    let id = song.id;

    // 音乐文件夹里的文件：直接读磁盘，不经过任何网络与缓存逻辑。
    if let Some(path) = local {
        let bytes = std::fs::read(&path)
            .map_err(|error| TrackError::Failed(format!("无法读取本地文件：{error}")))?;
        return Ok(LoadedTrack {
            bytes,
            label: "本地".to_string(),
        });
    }

    // 用户主动下载的文件最可靠：不受音质切换与缓存淘汰影响。
    if let Some((bytes, label)) = download_store::lookup(&downloads, id) {
        return Ok(LoadedTrack { bytes, label });
    }

    // 哔哩哔哩：DASH 纯音频流，音质档位由接口决定（忽略网易云的音质设置）。
    if song.is_bilibili() {
        let key = audio_cache::key(id, "bili");
        if let Some((bytes, label)) = audio_cache::lookup(&cache, &key) {
            return Ok(LoadedTrack { bytes, label });
        }

        let audio = bilibili
            .audio(&song.bvid, song.cid)
            .await
            .map_err(classify_track_error)?;
        let bytes = bilibili
            .fetch_bytes(&audio.url)
            .await
            .map_err(|error| TrackError::Failed(error.to_string()))?;
        audio_cache::store(&cache, &key, &bytes, &audio.label);
        return Ok(LoadedTrack {
            bytes,
            label: audio.label,
        });
    }

    let key = audio_cache::key(id, quality.value());
    if let Some((bytes, label)) = audio_cache::lookup(&cache, &key) {
        return Ok(LoadedTrack { bytes, label });
    }

    let song_url = client
        .song_url(id, quality.value())
        .await
        .map_err(classify_track_error)?;
    let url = song_url
        .available()
        .map(str::to_string)
        .ok_or_else(|| TrackError::Skipped(song_url.blocked_reason()))?;
    let bytes = client
        .fetch_bytes(&url)
        .await
        .map_err(|error| TrackError::Failed(error.to_string()))?;
    let label = song_url.quality_label();
    audio_cache::store(&cache, &key, &bytes, &label);
    Ok(LoadedTrack { bytes, label })
}

/// 下载一首歌并存进本地音乐库；返回实际拿到的音质标签。
async fn download_track(
    client: NeteaseClient,
    bilibili: BilibiliClient,
    store: Arc<Mutex<DownloadStore>>,
    song: Song,
    quality: AudioQuality,
) -> Result<String, TrackError> {
    let (bytes, label, kind) = if song.is_bilibili() {
        let audio = bilibili
            .audio(&song.bvid, song.cid)
            .await
            .map_err(classify_track_error)?;
        let bytes = bilibili
            .fetch_bytes(&audio.url)
            .await
            .map_err(|error| TrackError::Failed(error.to_string()))?;
        // DASH 音轨是 mp4 容器（fMP4），rodio / symphonia 可以直接解码。
        (bytes, audio.label, "m4s".to_string())
    } else {
        let info = client
            .song_url(song.id, quality.value())
            .await
            .map_err(classify_track_error)?;
        let url = info
            .available()
            .map(str::to_string)
            .ok_or_else(|| TrackError::Skipped(info.blocked_reason()))?;
        let bytes = client
            .fetch_bytes(&url)
            .await
            .map_err(|error| TrackError::Failed(error.to_string()))?;
        (bytes, info.quality_label(), info.kind.clone())
    };

    let mut store = store.lock().unwrap_or_else(|error| error.into_inner());
    store
        .add(song, &bytes, &label, &kind)
        .map_err(TrackError::Failed)?;
    Ok(label)
}

/// 区分「换一首就好」与「真的出错了」。
fn classify_track_error(error: ApiError) -> TrackError {
    match error {
        // VIP / 购买 / 下架 / 无版权：接口正常，只是没有音源。
        ApiError::Unavailable(reason) => TrackError::Skipped(reason),
        other => TrackError::Failed(other.to_string()),
    }
}

async fn load_stats(client: NeteaseClient, user_id: u64) -> Result<Option<UserStats>, String> {
    client
        .user_stats(user_id)
        .await
        .map_err(|error| error.to_string())
}

async fn load_avatar(client: NeteaseClient, dir: PathBuf, url: String) -> Result<Vec<u8>, String> {
    // 下载成功后顺便写进封面缓存；网络不可用时退回本地副本。
    let bytes = match client.fetch_bytes(&url).await {
        Ok(bytes) => {
            cover_cache::write(&dir, &url, &bytes);
            bytes
        }
        Err(error) => cover_cache::read(&dir, &url).ok_or_else(|| error.to_string())?,
    };
    avatar::round(&bytes, 320).ok_or_else(|| "头像解码失败".to_string())
}

async fn load_search(
    client: NeteaseClient,
    keyword: String,
    limit: u32,
    offset: u32,
) -> Result<SearchPage, String> {
    client
        .search_songs(&keyword, limit, offset)
        .await
        .map_err(|error| error.to_string())
}

async fn load_daily_songs(client: NeteaseClient) -> Result<Vec<Song>, String> {
    client
        .daily_songs()
        .await
        .map_err(|error| error.to_string())
}

/// 一次取 12 首就够撑满页面；接口每次只给 3 首，内部会多拉几轮。
const FM_BATCH: u32 = 12;
/// 歌手页每次加载的歌曲数（接口实测支持 200，这里取 100 控制渲染量）。
const ARTIST_PAGE: u32 = 100;

async fn load_artist(client: NeteaseClient, id: u64) -> Result<(ArtistDetail, Vec<Song>), String> {
    let detail = client
        .artist_detail(id)
        .await
        .map_err(|error| error.to_string())?;
    let songs = load_artist_songs(client, id, 0).await?;
    Ok((detail, songs))
}

async fn load_artist_songs(
    client: NeteaseClient,
    id: u64,
    offset: u32,
) -> Result<Vec<Song>, String> {
    client
        .artist_songs(id, ARTIST_PAGE, offset)
        .await
        .map_err(|error| error.to_string())
}

async fn load_fm_songs(client: NeteaseClient) -> Result<Vec<Song>, String> {
    client
        .personal_fm(FM_BATCH)
        .await
        .map_err(|error| error.to_string())
}

async fn load_playlists(
    client: NeteaseClient,
    user_id: u64,
) -> Result<Vec<PlaylistSummary>, String> {
    client
        .user_playlists(user_id)
        .await
        .map_err(|error| error.to_string())
}

async fn load_playlist(client: NeteaseClient, id: u64) -> Result<PlaylistDetail, String> {
    client
        .playlist_detail(id)
        .await
        .map_err(|error| error.to_string())
}

/// 用粘贴的 Cookie 验证 B 站登录态。验证失败会清空客户端里的 Cookie。
async fn connect_bilibili(client: BilibiliClient, input: String) -> Result<BiliUser, String> {
    let cookies = nm_api::parse_bilibili_cookie_input(&input);
    let has_sessdata = cookies
        .get("SESSDATA")
        .is_some_and(|value| !value.is_empty());
    if !has_sessdata {
        return Err("没有找到 SESSDATA，请粘贴完整 Cookie 或 SESSDATA 的值。".to_string());
    }

    let previous = client.snapshot_cookies();
    client.set_cookies(cookies);
    match client.user().await {
        Ok(Some(user)) => Ok(user),
        Ok(None) => {
            client.set_cookies(previous);
            Err("Cookie 无效或已过期，请重新登录 bilibili.com 后再复制。".to_string())
        }
        Err(error) => {
            client.set_cookies(previous);
            Err(error.to_string())
        }
    }
}

async fn fetch_bili_folders(client: BilibiliClient, mid: u64) -> Result<Vec<BiliFolder>, String> {
    client.folders(mid).await.map_err(|error| error.to_string())
}

/// 收藏夹列表不返回封面，单独取第一页第一条视频的封面；失败时返回空串。
async fn fetch_bili_folder_cover(client: BilibiliClient, id: u64) -> String {
    client.folder_cover(id).await.unwrap_or_default()
}

async fn fetch_bili_folder(
    client: BilibiliClient,
    id: u64,
    page: u32,
) -> Result<BiliFolderPage, String> {
    client
        .folder(id, page)
        .await
        .map_err(|error| error.to_string())
}

/// 把一页收藏夹内容合并进已打开的列表；返回需要预取封面的 URL。
fn apply_bili_page(
    state: &mut PlaylistState,
    id: u64,
    page: u32,
    page_data: BiliFolderPage,
) -> Vec<String> {
    let first = page == 1;
    let mut covers: Vec<String> = page_data.songs.iter().map(|song| song.cover(96)).collect();

    let detail = state.detail.get_or_insert_with(|| PlaylistDetail {
        id,
        name: page_data.title.clone(),
        cover_img_url: page_data.cover.clone(),
        track_count: page_data.total,
        play_count: 0,
        description: None,
        creator: None,
        subscribed: false,
        tracks: Vec::new(),
        track_ids: Vec::new(),
    });

    if first {
        detail.name = page_data.title;
        detail.cover_img_url = page_data.cover.clone();
        detail.tracks.clear();
        detail.track_count = page_data.total;
        detail.description =
            (!page_data.description.trim().is_empty()).then_some(page_data.description.clone());
        if !detail.cover_img_url.is_empty() {
            covers.push(detail.cover(420));
        }
    }

    let known: HashSet<u64> = detail.tracks.iter().map(|song| song.id).collect();
    detail.tracks.extend(
        page_data
            .songs
            .into_iter()
            .filter(|song| !known.contains(&song.id)),
    );
    detail.track_count = detail
        .track_count
        .max(page_data.total)
        .max(detail.tracks.len() as u64);

    state.bili_page = page;
    state.bili_has_more = page_data.has_more;
    state.bili_busy = false;
    state.busy = false;
    state.error = None;
    state.filling = false;

    covers
}

/// 详情接口只返回歌单前一部分歌曲；用 `trackIds` 分批补齐剩下的。
async fn fill_playlist_tracks(
    client: NeteaseClient,
    missing: Vec<u64>,
) -> Result<Vec<Song>, String> {
    let mut songs = Vec::with_capacity(missing.len());
    for chunk in missing.chunks(300) {
        match client.songs_by_ids(chunk).await {
            Ok(mut batch) => songs.append(&mut batch),
            Err(error) => {
                // 已经拿到多少就显示多少：补齐失败不该让整个歌单打不开。
                if songs.is_empty() {
                    return Err(error.to_string());
                }
                break;
            }
        }
    }
    Ok(songs)
}

/// 歌单歌曲列表的可视窗口：行高固定，只渲染滚动到眼前的那一段。
fn playlist_window(viewport_height: f32, offset: f32, total: usize) -> (usize, usize) {
    let view_height = viewport_height.max(views::home::TRACK_ROW_HEIGHT);
    let (first, last, ..) = views::scroll_window::visible_window(
        total,
        offset,
        view_height,
        views::home::TRACK_ROW_HEIGHT,
        views::home::TRACK_OVERSCAN,
    );
    (first, last)
}

/// 歌单内搜索关键词变化后，把歌单列表滚回顶部。
fn scroll_playlist_to_top() -> Task<Message> {
    iced::widget::scrollable::scroll_to(
        iced::widget::scrollable::Id::new(views::home::PLAYLIST_SCROLL_ID),
        iced::widget::scrollable::AbsoluteOffset { x: 0.0, y: 0.0 },
    )
}

/// 从本地缓存读封面并裁好；缓存里没有时返回错误（等网络刷新）。
async fn load_cached_cover(dir: PathBuf, url: String) -> Result<CoverImage, String> {
    let bytes = cover_cache::read(&dir, &url).ok_or_else(|| "封面缓存未命中".to_string())?;
    Ok(decorate_cover(&url, bytes))
}

/// 本地音乐文件的内嵌封面：先看缓存，没有再读标签里的图片。
async fn load_local_cover(dir: PathBuf, url: String, path: PathBuf) -> Result<CoverImage, String> {
    if let Some(bytes) = cover_cache::read(&dir, &url) {
        return Ok(decorate_cover(&url, bytes));
    }
    let bytes =
        local_files::read_cover(&path).ok_or_else(|| "本地歌曲没有内嵌封面。".to_string())?;
    cover_cache::write(&dir, &url, &bytes);
    Ok(decorate_cover(&url, bytes))
}

/// 下载封面、写入缓存，再裁好交给界面。
async fn load_cover(
    client: NeteaseClient,
    bilibili: BilibiliClient,
    dir: PathBuf,
    url: String,
) -> Result<CoverImage, String> {
    // 哔哩哔哩的图片 CDN 需要 Referer，交给 B 站客户端下载。
    let bytes = if nm_api::is_bilibili_url(&url) {
        bilibili
            .fetch_bytes(&url)
            .await
            .map_err(|error| error.to_string())?
    } else {
        client
            .fetch_bytes(&url)
            .await
            .map_err(|error| error.to_string())?
    };
    cover_cache::write(&dir, &url, &bytes);
    Ok(decorate_cover(&url, bytes))
}

/// 拿到原图后裁圆角（列表用），播放页大封面额外裁一份正圆。
fn decorate_cover(url: &str, bytes: Vec<u8>) -> CoverImage {
    let size = cover_size(url);
    let rounded = size.and_then(|size| avatar::rounded(&bytes, size.clamp(16, 512), 0.12));
    let circle = (size == Some(600))
        .then(|| avatar::round(&bytes, 600))
        .flatten();

    CoverImage {
        bytes: rounded.unwrap_or(bytes),
        circle,
    }
}

/// 从封面 URL 取出目标尺寸。
///
/// * 网易云：`param=300y300`；
/// * 哔哩哔哩：`@300w_300h_1c`。
fn cover_size(url: &str) -> Option<u32> {
    if let Some(value) = url.split_once("param=").map(|(_, value)| value) {
        if let Some(size) = value.split('y').next().and_then(|size| size.parse().ok()) {
            return Some(size);
        }
    }

    let value = url.split_once('@')?.1;
    let width = value.split('w').next()?;
    width.parse().ok()
}

#[cfg(test)]
mod cover_size_tests {
    use super::cover_size;

    #[test]
    fn reads_both_cdn_styles() {
        assert_eq!(
            cover_size("https://p1.music.126.net/a.jpg?param=300y300"),
            Some(300)
        );
        assert_eq!(
            cover_size("https://i0.hdslb.com/bfs/archive/a.jpg@600w_600h_1c"),
            Some(600)
        );
        assert_eq!(cover_size("https://i0.hdslb.com/bfs/archive/a.jpg"), None);
    }
}

/// 仅用于开发期布局预览：MELO_DEMO_USER=1。
fn demo_profile() -> Profile {
    Profile {
        user_id: 42_424_242,
        nickname: "云村听歌人".to_string(),
        signature: "在旋律里散步，也在代码里散步。".to_string(),
        vip_type: 11,
        level: 9,
        follows: 42,
        followeds: 128,
        playlist_count: 12,
        event_count: 3,
        ..Profile::default()
    }
}

/// 演示数据：歌手页，不依赖网络。
fn demo_artist(id: u64) -> ArtistDetail {
    ArtistDetail {
        id,
        name: "周杰伦".to_string(),
        alias: vec!["Jay Chou".to_string(), "周董".to_string()],
        brief_desc: "华语流行乐男歌手、音乐人、演员、导演、编剧。2000 年发行首张专辑《Jay》正式出道，\n以融合中国风与节奏蓝调的创作风格被大众熟知。".to_string(),
        music_size: demo_songs().len() as u64,
        album_size: 14,
        mv_size: 9,
        ..ArtistDetail::default()
    }
}

fn demo_playlists() -> Vec<PlaylistSummary> {
    const NAMES: &[(&str, u64, bool)] = &[
        ("深夜循环", 36, false),
        ("通勤路上", 24, false),
        ("写字时听", 18, true),
        ("最近收藏", 42, true),
    ];

    NAMES
        .iter()
        .enumerate()
        .map(|(index, (name, tracks, subscribed))| PlaylistSummary {
            id: 1000 + index as u64,
            name: (*name).to_string(),
            cover_img_url: String::new(),
            track_count: *tracks,
            play_count: 1200 * (index as u64 + 1),
            creator: Some(Creator {
                user_id: 42_424_242,
                nickname: "云村听歌人".to_string(),
            }),
            subscribed: *subscribed,
            special_type: 0,
        })
        .collect()
}

fn demo_detail(id: u64) -> PlaylistDetail {
    let summary = demo_playlists()
        .into_iter()
        .find(|item| item.id == id)
        .unwrap_or_default();

    PlaylistDetail {
        id: summary.id,
        name: summary.name,
        cover_img_url: summary.cover_img_url,
        track_count: summary.track_count,
        play_count: summary.play_count,
        description: Some("演示用歌单，登录后会替换成你的真实收藏。".to_string()),
        creator: summary.creator,
        subscribed: summary.subscribed,
        tracks: demo_songs(),
        track_ids: Vec::new(),
    }
}

fn demo_songs() -> Vec<Song> {
    const SONGS: &[(&str, &str, &str, u64)] = &[
        ("晴天", "周杰伦", "叶惠美", 269_000),
        ("City of Stars", "Ryan Gosling", "La La Land", 149_000),
        ("海阔天空", "Beyond", "乐与怒", 326_000),
        ("起风了", "买辣椒也用券", "起风了", 325_000),
        ("夜曲", "周杰伦", "十一月的萧邦", 226_000),
        ("Lemon", "米津玄師", "Lemon", 255_000),
    ];

    SONGS
        .iter()
        .enumerate()
        .map(|(index, (name, artist, album, duration))| Song {
            id: 2000 + index as u64,
            name: (*name).to_string(),
            ar: vec![Artist {
                id: index as u64 + 1,
                name: (*artist).to_string(),
            }],
            al: Some(Album {
                id: index as u64 + 10,
                name: (*album).to_string(),
                pic_url: String::new(),
            }),
            dt: *duration,
            fee: 0,
            ..Song::default()
        })
        .collect()
}

/// 显式加载系统中文字体。iced 的字体枚举在部分 Linux 环境下不可靠，
/// 直接指定字体文件能保证中文标签不会变成方框。
/// 缺字（emoji、符号）的回退由 `widgets::text` 的 `Shaping::Advanced` 负责。
fn system_cjk_font() -> (Font, Vec<Cow<'static, [u8]>>) {
    const CANDIDATES: &[(&str, &str)] = &[
        (
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "Noto Sans CJK SC",
        ),
        (
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "Noto Sans CJK SC",
        ),
        (
            "/usr/share/fonts/wenquanyi/wqy-zenhei/wqy-zenhei.ttc",
            "WenQuanYi Zen Hei",
        ),
        (
            "/usr/share/fonts/wenquanyi/wqy-microhei/wqy-microhei.ttc",
            "WenQuanYi Micro Hei",
        ),
        (
            "/usr/share/fonts/TTF/wqy-microhei.ttc",
            "WenQuanYi Micro Hei",
        ),
        ("/usr/share/fonts/TTF/uming.ttc", "AR PL UMing CN"),
    ];

    for (path, family) in CANDIDATES {
        if let Ok(bytes) = std::fs::read(path) {
            return (Font::with_name(family), vec![Cow::Owned(bytes)]);
        }
    }

    (Font::DEFAULT, Vec::new())
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    fn queue(ids: &[u64]) -> Vec<Song> {
        ids.iter()
            .map(|id| Song {
                id: *id,
                ..Song::default()
            })
            .collect()
    }

    fn mode(shuffle: bool, repeat: RepeatMode) -> PlayMode {
        PlayMode { shuffle, repeat }
    }

    fn ids(songs: &[Song]) -> Vec<u64> {
        songs.iter().map(|song| song.id).collect()
    }

    fn song(id: u64) -> Song {
        Song {
            id,
            ..Song::default()
        }
    }

    #[test]
    fn removing_before_current_shifts_index() {
        let mut songs = queue(&[1, 2, 3, 4]);
        let mut index = 2;

        assert_eq!(
            remove_queue_entry(&mut songs, &mut index, 1),
            QueueChange::Keep
        );
        assert_eq!(ids(&songs), [1, 3, 4]);
        assert_eq!(index, 1, "当前歌曲仍然是 3");
    }

    #[test]
    fn removing_current_plays_the_next_entry() {
        let mut songs = queue(&[1, 2, 3, 4]);
        let mut index = 1;

        assert_eq!(
            remove_queue_entry(&mut songs, &mut index, 1),
            QueueChange::Play
        );
        assert_eq!(ids(&songs), [1, 3, 4]);
        assert_eq!(index, 1, "位置 1 现在是原来的 3");
    }

    #[test]
    fn removing_current_last_entry_falls_back_to_previous() {
        let mut songs = queue(&[1, 2, 3]);
        let mut index = 2;

        assert_eq!(
            remove_queue_entry(&mut songs, &mut index, 2),
            QueueChange::Play
        );
        assert_eq!(ids(&songs), [1, 2]);
        assert_eq!(index, 1);
    }

    #[test]
    fn removing_after_current_keeps_index() {
        let mut songs = queue(&[1, 2, 3]);
        let mut index = 1;

        assert_eq!(
            remove_queue_entry(&mut songs, &mut index, 2),
            QueueChange::Keep
        );
        assert_eq!(ids(&songs), [1, 2]);
        assert_eq!(index, 1);
    }

    #[test]
    fn removing_the_only_entry_stops_playback() {
        let mut songs = queue(&[1]);
        let mut index = 0;

        assert_eq!(
            remove_queue_entry(&mut songs, &mut index, 0),
            QueueChange::Stop
        );
        assert!(songs.is_empty());
        assert_eq!(index, 0);
    }

    #[test]
    fn out_of_range_remove_is_ignored() {
        let mut songs = queue(&[1, 2]);
        let mut index = 1;

        assert_eq!(
            remove_queue_entry(&mut songs, &mut index, 9),
            QueueChange::Keep
        );
        assert_eq!(ids(&songs), [1, 2]);
        assert_eq!(index, 1);
    }

    #[test]
    fn sequential_advances_until_the_end() {
        assert_eq!(next_index(mode(false, RepeatMode::Off), 3, 0), Some(1));
        assert_eq!(next_index(mode(false, RepeatMode::Off), 3, 2), None);
    }

    #[test]
    fn repeat_all_wraps_to_the_start() {
        assert_eq!(next_index(mode(false, RepeatMode::All), 3, 0), Some(1));
        assert_eq!(next_index(mode(false, RepeatMode::All), 3, 2), Some(0));
    }

    #[test]
    fn repeat_one_stays_on_the_same_song() {
        assert_eq!(next_index(mode(false, RepeatMode::One), 3, 1), Some(1));
    }

    #[test]
    fn empty_queue_has_no_next() {
        for shuffle in [false, true] {
            for repeat in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
                assert_eq!(next_index(mode(shuffle, repeat), 0, 0), None);
            }
        }
    }

    #[test]
    fn shuffle_picks_another_song_and_ignores_sequential_order() {
        let next = next_index(mode(true, RepeatMode::Off), 10, 4).unwrap();
        assert!(next < 10);
        assert_ne!(next, 4);
    }

    #[test]
    fn single_loop_wins_over_shuffle() {
        assert_eq!(next_index(mode(true, RepeatMode::One), 10, 4), Some(4));
    }

    #[test]
    fn manual_next_skips_the_song_even_in_repeat_one() {
        let one = mode(false, RepeatMode::One);
        assert_eq!(skip_index(one, 3, 0), Some(1));
        assert_eq!(skip_index(one, 3, 1), Some(2));
        // 单曲循环也算“还在循环”：队尾手动切歌回到队首。
        assert_eq!(skip_index(one, 3, 2), Some(0));
    }

    #[test]
    fn manual_next_stops_at_the_end_without_any_repeat() {
        assert_eq!(skip_index(mode(false, RepeatMode::Off), 3, 0), Some(1));
        assert_eq!(skip_index(mode(false, RepeatMode::Off), 3, 2), None);
        // 列表循环与自动续播一致：回到队首。
        assert_eq!(skip_index(mode(false, RepeatMode::All), 3, 2), Some(0));
    }

    #[test]
    fn manual_next_prefers_shuffle_over_repeat_one() {
        let next = skip_index(mode(true, RepeatMode::One), 10, 4).unwrap();
        assert!(next < 10);
        assert_ne!(next, 4);
    }

    #[test]
    fn manual_next_with_an_empty_queue_does_nothing() {
        for shuffle in [false, true] {
            for repeat in [RepeatMode::Off, RepeatMode::All, RepeatMode::One] {
                assert_eq!(skip_index(mode(shuffle, repeat), 0, 0), None);
            }
        }
    }

    #[test]
    fn shuffle_and_repeat_all_can_be_combined() {
        let next = next_index(mode(true, RepeatMode::All), 2, 0).unwrap();
        assert_eq!(next, 1);
    }

    #[test]
    fn legacy_mode_strings_still_load() {
        let legacy: PlayMode = serde_json::from_str("\"shuffle\"").unwrap();
        assert_eq!(legacy, mode(true, RepeatMode::Off));

        let repeat: PlayMode = serde_json::from_str("\"repeat_all\"").unwrap();
        assert_eq!(repeat, mode(false, RepeatMode::All));

        let single: PlayMode = serde_json::from_str("\"repeat_one\"").unwrap();
        assert_eq!(single, mode(false, RepeatMode::One));

        let sequential: PlayMode = serde_json::from_str("\"sequential\"").unwrap();
        assert_eq!(sequential, PlayMode::default());
    }

    #[test]
    fn split_mode_round_trips() {
        let mode = mode(true, RepeatMode::One);
        let json = serde_json::to_string(&mode).unwrap();
        let loaded: PlayMode = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded, mode);
    }

    #[test]
    fn shuffle_single_entry_replays_itself() {
        assert_eq!(shuffle_index(0, 0), None);
        assert_eq!(shuffle_index(1, 0), Some(0));
    }

    #[test]
    fn shuffle_history_round_trips_the_previous_song() {
        let mut history = Vec::new();
        push_history(&mut history, 3);
        push_history(&mut history, 7);

        assert_eq!(pop_history(&mut history, 10, 2), Some(7));
        assert_eq!(pop_history(&mut history, 10, 2), Some(3));
        assert_eq!(pop_history(&mut history, 10, 2), None);
    }

    #[test]
    fn shuffle_history_does_not_repeat_the_same_index() {
        let mut history = Vec::new();
        push_history(&mut history, 3);
        push_history(&mut history, 3);

        assert_eq!(history, [3]);
        assert_eq!(pop_history(&mut history, 10, 0), Some(3));
        assert_eq!(pop_history(&mut history, 10, 0), None);
    }

    #[test]
    fn shuffle_history_skips_stale_or_current_entries() {
        // 索引 8 已经被移除，2 是当前曲；两者都跳过。
        assert_eq!(pop_history(&mut vec![2, 5, 8], 7, 2), Some(5));
    }

    #[test]
    fn shuffle_history_is_capped() {
        let mut history = Vec::new();
        for index in 0..MAX_HISTORY + 10 {
            push_history(&mut history, index);
        }

        assert_eq!(history.len(), MAX_HISTORY);
        assert_eq!(history.first(), Some(&10));
        assert_eq!(history.last(), Some(&(MAX_HISTORY + 9)));
    }

    #[test]
    fn removing_a_queue_entry_keeps_history_indices_in_sync() {
        let mut history = vec![0, 2, 4];
        drop_history_entry(&mut history, 2);
        assert_eq!(history, [0, 3]);
    }

    #[test]
    fn play_next_inserts_right_after_the_current_track() {
        let mut songs = queue(&[1, 2, 3]);
        let mut history = Vec::new();

        let position = insert_after_current(&mut songs, &mut history, 0, song(7));

        assert_eq!(position, 1);
        assert_eq!(ids(&songs), [1, 7, 2, 3]);
    }

    #[test]
    fn successive_play_next_keeps_the_last_added_first() {
        // A 正在播放：先加 H，再加 K，顺序应为 A K H B C。
        let mut songs = queue(&[1, 2, 3]);
        let mut history = Vec::new();

        insert_after_current(&mut songs, &mut history, 0, song(8));
        insert_after_current(&mut songs, &mut history, 0, song(9));

        assert_eq!(ids(&songs), [1, 9, 8, 2, 3]);
    }

    #[test]
    fn play_next_at_the_queue_end_appends() {
        let mut songs = queue(&[1, 2, 3]);
        let mut history = Vec::new();

        let position = insert_after_current(&mut songs, &mut history, 2, song(4));

        assert_eq!(position, 3);
        assert_eq!(ids(&songs), [1, 2, 3, 4]);
    }

    #[test]
    fn play_next_shifts_shuffle_history_indices() {
        let mut songs = queue(&[1, 2, 3]);
        // 随机历史里 0 是当前曲、2 是更早的一首；插入后 2 应该后移成 3。
        let mut history = vec![0, 2];

        insert_after_current(&mut songs, &mut history, 0, song(7));

        assert_eq!(history, [0, 3]);
    }

    #[test]
    fn scrolling_up_raises_volume_and_clamps() {
        let up = mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 };
        assert!((volume_after_scroll(0.5, up) - 0.55).abs() < 1e-6);
        assert_eq!(volume_after_scroll(1.0, up), 1.0);
    }

    #[test]
    fn scrolling_down_lowers_volume_and_clamps() {
        let down = mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 };
        assert!((volume_after_scroll(0.5, down) - 0.45).abs() < 1e-6);
        assert_eq!(volume_after_scroll(0.0, down), 0.0);
    }

    #[test]
    fn pixel_scroll_is_more_gentle_than_line_scroll() {
        let lines = mouse::ScrollDelta::Lines { x: 0.0, y: 1.0 };
        let pixels = mouse::ScrollDelta::Pixels { x: 0.0, y: 1.0 };
        assert!(volume_after_scroll(0.5, lines) > volume_after_scroll(0.5, pixels));
    }

    #[test]
    fn clicking_volume_mutes_and_restores_the_previous_level() {
        let mut volume = 0.6;
        let mut muted = None;

        toggle_mute(&mut volume, &mut muted);
        assert_eq!(volume, 0.0);
        assert_eq!(muted, Some(0.6));

        toggle_mute(&mut volume, &mut muted);
        assert_eq!(volume, 0.6);
        assert_eq!(muted, None);
    }

    #[test]
    fn muting_an_already_zero_volume_restores_the_default() {
        let mut volume = 0.0;
        let mut muted = None;

        toggle_mute(&mut volume, &mut muted);
        assert_eq!(volume, DEFAULT_VOLUME);
        assert_eq!(muted, None);
    }

    #[test]
    fn restored_track_clamps_index_and_position() {
        let snapshot = PlayerSnapshot {
            queue: vec![
                Song {
                    id: 1,
                    dt: 200_000,
                    ..Song::default()
                },
                Song {
                    id: 2,
                    dt: 180_000,
                    ..Song::default()
                },
            ],
            index: 99,
            position: 999.0,
            ..PlayerSnapshot::default()
        };

        let (index, duration, position) = restored_track(&snapshot).unwrap();
        assert_eq!(index, 1);
        assert_eq!(duration, Duration::from_secs(180));
        assert_eq!(position, Duration::from_secs(180));
    }

    #[test]
    fn restored_track_is_empty_without_a_queue() {
        assert!(restored_track(&PlayerSnapshot::default()).is_none());
    }

    #[test]
    fn approach_moves_toward_the_target_without_overshooting() {
        assert!((approach(0.0, 1.0, 0.3) - 0.3).abs() < 1e-6);
        assert_eq!(approach(0.9, 1.0, 0.3), 1.0);
    }

    #[test]
    fn approach_moves_back_down_when_target_drops() {
        assert!((approach(1.0, 0.0, 0.25) - 0.75).abs() < 1e-6);
        assert_eq!(approach(0.1, 0.0, 0.25), 0.0);
    }

    #[test]
    fn fm_refill_appends_only_unseen_songs() {
        let mut songs = queue(&[1, 2, 3]);
        // 新一批里既有队列已有的 2、3，也有重复的 4，只接 4、5 两首。
        let start = append_unique(&mut songs, queue(&[2, 3, 4, 4, 5]));
        assert_eq!(start, Some(3));
        assert_eq!(ids(&songs), [1, 2, 3, 4, 5]);
    }

    #[test]
    fn fm_refill_reports_nothing_new() {
        let mut songs = queue(&[1, 2]);
        assert_eq!(append_unique(&mut songs, queue(&[2, 1])), None);
        assert_eq!(ids(&songs), [1, 2]);
    }

    #[test]
    fn toast_fades_in_holds_then_fades_out() {
        assert_eq!(toast_opacity(Duration::ZERO), 0.0);
        assert!((toast_opacity(Duration::from_millis(80)) - 0.5).abs() < 0.01);
        assert_eq!(toast_opacity(Duration::from_millis(1000)), 1.0);
        assert!(toast_opacity(Duration::from_millis(2300)) < 1.0);
        assert_eq!(toast_opacity(TOAST_LIFE), 0.0);
    }

    #[test]
    fn toast_text_shortens_long_titles() {
        assert_eq!(shorten("晴天", 16), "晴天");
        assert_eq!(
            shorten("一二三四五六七八九十一二三四五六七", 5),
            "一二三四五…"
        );
    }
}

#[cfg(test)]
mod scroll_tests {
    use super::*;

    #[test]
    fn transfer_paths_are_trimmed_and_accept_plain_paths() {
        assert_eq!(
            expand_path("  /tmp/liked.json  "),
            PathBuf::from("/tmp/liked.json")
        );
        assert!(expand_path("   ").as_os_str().is_empty());
        assert_eq!(expand_path("relative.json"), PathBuf::from("relative.json"));
    }

    #[test]
    fn scrollbars_start_hidden_and_hide_again_after_a_pause() {
        let now = Instant::now();
        let recent = now.checked_sub(Duration::from_millis(500)).unwrap();
        let stale = now.checked_sub(Duration::from_millis(700)).unwrap();

        // 默认（从未滚动）是隐藏的。
        assert!(!scroll_recent(None, now));
        // 0.6 秒内滚过：显示。
        assert!(scroll_recent(Some(recent), now));
        assert!(scroll_recent(Some(now), now));
        // 停下来超过 0.6 秒：重新隐藏。
        assert!(!scroll_recent(Some(stale), now));
    }
}

//! Montis（山音）的本地数据层。
//!
//! 这里不碰网络，也不碰 UI：只负责 Cookie 会话、用户资料模型和本地路径。
//! 这样登录流程可以脱离窗口单独测试，后续的歌单 / 播放缓存也能复用同一套目录约定。

pub mod bilibili;
pub mod cookies;
pub mod model;
pub mod paths;
pub mod session;

pub use bilibili::BilibiliSession;
pub use cookies::{apply_set_cookie, cookie_header, cookie_value, parse_cookie_input, Cookies};
pub use model::Profile;
pub use paths::AppPaths;
pub use session::Session;

//! Cookie 会话：网易云音乐的登录态完全由一组 Cookie 承载。
//!
//! 网易云在登录成功时通过 `Set-Cookie` 下发 `MUSIC_U`（真正的登录票据）、
//! `__csrf`（请求校验）等字段。这里用有序 map 保存，既便于持久化，
//! 也便于按固定顺序拼出 `Cookie:` 请求头。

use std::collections::BTreeMap;

pub type Cookies = BTreeMap<String, String>;

/// 解析用户粘贴的 Cookie 字符串。
///
/// 兼容三种常见形式：
/// * 完整的 `MUSIC_U=...; __csrf=...`；
/// * 从开发者工具里只复制的 `MUSIC_U` 值；
/// * 带换行 / 多余空格的粘贴内容。
pub fn parse_cookie_input(input: &str) -> Cookies {
    let normalized = input.replace(['\r', '\n'], ";");
    let trimmed = normalized.trim();
    let mut cookies = Cookies::new();
    if trimmed.is_empty() {
        return cookies;
    }

    // 只粘贴了一个值：按 MUSIC_U 处理。
    if !trimmed.contains('=') {
        cookies.insert("MUSIC_U".to_string(), trimmed.trim_matches('"').to_string());
        return cookies;
    }

    for part in trimmed.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some((name, value)) = part.split_once('=') else {
            continue;
        };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let value = value.trim().trim_matches('"');
        cookies.insert(name.to_string(), value.to_string());
    }

    cookies
}

/// 把一条 `Set-Cookie` 响应头合并进会话。
///
/// 只关心键值本身；`Expires` / `Path` / `Domain` 这些属性对本地单域客户端没有意义。
/// `Max-Age=0`（或负数）代表服务端要求删除该 Cookie，需要同步移除。
pub fn apply_set_cookie(cookies: &mut Cookies, raw: &str) {
    let mut segments = raw.split(';');
    let Some(pair) = segments.next() else {
        return;
    };
    let Some((name, value)) = pair.split_once('=') else {
        return;
    };
    let name = name.trim();
    if name.is_empty() {
        return;
    }

    let deleted = segments.any(|attribute| {
        let attribute = attribute.trim();
        let lower = attribute.to_ascii_lowercase();
        lower.starts_with("max-age=")
            && lower
                .trim_start_matches("max-age=")
                .trim()
                .parse::<i64>()
                .is_ok_and(|seconds| seconds <= 0)
    });

    if deleted {
        cookies.remove(name);
    } else {
        cookies.insert(name.to_string(), value.trim().to_string());
    }
}

/// 拼出请求用的 `Cookie` 头；没有 Cookie 时返回空串。
pub fn cookie_header(cookies: &Cookies) -> String {
    cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ")
}

pub fn cookie_value<'a>(cookies: &'a Cookies, name: &str) -> Option<&'a str> {
    cookies.get(name).map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_cookie_string() {
        let cookies = parse_cookie_input("MUSIC_U=abc==; __csrf=xyz; os=pc");
        assert_eq!(cookies.get("MUSIC_U").map(String::as_str), Some("abc=="));
        assert_eq!(cookies.get("__csrf").map(String::as_str), Some("xyz"));
        assert_eq!(cookies.get("os").map(String::as_str), Some("pc"));
    }

    #[test]
    fn bare_value_is_treated_as_music_u() {
        let cookies = parse_cookie_input("  1234567890  ");
        assert_eq!(cookies.len(), 1);
        assert_eq!(
            cookies.get("MUSIC_U").map(String::as_str),
            Some("1234567890")
        );
    }

    #[test]
    fn multiline_paste_is_normalized() {
        let cookies = parse_cookie_input("MUSIC_U=a\n__csrf=b\r\n");
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies.get("__csrf").map(String::as_str), Some("b"));
    }

    #[test]
    fn set_cookie_merges_and_removes() {
        let mut cookies = Cookies::new();
        apply_set_cookie(&mut cookies, "MUSIC_U=first; Path=/; HttpOnly");
        apply_set_cookie(&mut cookies, "NMTID=second; Max-Age=3600");
        assert_eq!(cookie_header(&cookies), "MUSIC_U=first; NMTID=second");

        apply_set_cookie(&mut cookies, "MUSIC_U=; Max-Age=0; Path=/");
        assert_eq!(cookies.get("MUSIC_U"), None);
    }
}

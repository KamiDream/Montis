//! Cookie 会话：网易云音乐的登录态完全由一组 Cookie 承载。
//!
//! 网易云在登录成功时通过 `Set-Cookie` 下发 `MUSIC_U`（真正的登录票据）、
//! `__csrf`（请求校验）等字段。这里用有序 map 保存，既便于持久化，
//! 也便于按固定顺序拼出 `Cookie:` 请求头。

use std::collections::BTreeMap;

pub type Cookies = BTreeMap<String, String>;

/// 解析用户粘贴的 Cookie 字符串。
///
/// 兼容几种常见形式：
/// * 完整的 `MUSIC_U=...; __csrf=...`；
/// * 从开发者工具里只复制的 `MUSIC_U` 值；
/// * 带换行 / 多余空格的粘贴内容；
/// * 从开发者工具 Cookies 表格整行 / 整表复制的内容（制表符分列）。
pub fn parse_cookie_input(input: &str) -> Cookies {
    let mut cookies = Cookies::new();
    let input = strip_cookie_label(input.trim());
    let input = input.trim_matches('"');

    for line in input.split(['\r', '\n']) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Cookies 表格的每一行是 `名称\t值\t域\t路径…`。整行复制时按列取
        // 前两列；否则会把域名、路径也当成独立的 Cookie。
        if line.contains('\t') {
            if let Some((name, value)) = table_row(line) {
                cookies.insert(name, value);
            }
            continue;
        }

        // 普通 Cookie 头：同一行里用 `;` 分隔多个键值。
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

            // 没有 `=`：要么是单独复制的值，要么是行复制时制表符已经被
            // 输入框吞掉的 `MUSIC_U<值>域名`。两者都按 MUSIC_U 处理。
            let value = recover_music_u(part).unwrap_or_else(|| clean_value(part));
            if !value.is_empty() {
                cookies.insert("MUSIC_U".to_string(), value);
            }
        }
    }

    cookies
}

/// 去掉 `Cookie:` / `Set-Cookie:` 这类复制自请求头的前缀。
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

/// Cookie 名称允许的字符；用于排除 `Cookie:` 残留、域名等无效条目。
fn is_cookie_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn clean_value(value: &str) -> String {
    value.trim().trim_matches('"').to_string()
}

fn table_row(line: &str) -> Option<(String, String)> {
    let mut columns = line.split('\t').map(str::trim);
    let name = columns.next()?.trim_end_matches(':');
    let value = columns.next().unwrap_or_default();
    if !is_cookie_name(name) || value.is_empty() {
        return None;
    }
    Some((name.to_string(), clean_value(value)))
}

/// 输入框（iced 的 `text_input`）粘贴时会丢弃制表符与换行，所以从
/// Cookies 表格整行复制后可能只剩 `MUSIC_U<值>music.163.com/…`。
/// `MUSIC_U` 的值是十六进制串，遇到域名里的字母就能截断。
fn recover_music_u(token: &str) -> Option<String> {
    let rest = token.strip_prefix("MUSIC_U")?;
    let value: String = rest
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() || *c == '=')
        .collect();

    // 太短的内容多半只是误粘贴，不要当凭证。
    (value.len() >= 16).then_some(value)
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
    fn header_label_is_stripped() {
        let cookies = parse_cookie_input("Cookie: MUSIC_U=abc; __csrf=xyz");
        assert_eq!(cookies.get("MUSIC_U").map(String::as_str), Some("abc"));
        assert_eq!(cookies.get("__csrf").map(String::as_str), Some("xyz"));
        assert_eq!(cookies.len(), 2);
    }

    #[test]
    fn devtools_table_rows_are_parsed() {
        // 从 DevTools 的 Cookies 表格整段复制：制表符分列，每行一个 Cookie。
        let cookies = parse_cookie_input(
            "MUSIC_U\t00AB12CD34EF56AB78CD90EF12AB34CD\t.music.163.com\t/\t2027-09-01\n\
             __csrf\tdeadbeef\t.music.163.com\t/\tSession",
        );
        assert_eq!(
            cookies.get("MUSIC_U").map(String::as_str),
            Some("00AB12CD34EF56AB78CD90EF12AB34CD")
        );
        assert_eq!(cookies.get("__csrf").map(String::as_str), Some("deadbeef"));
        assert_eq!(cookies.len(), 2);
    }

    #[test]
    fn music_u_copied_as_row_is_recovered() {
        // 输入框吞掉制表符后，整行复制会变成 `MUSIC_U<值>域名`。
        let cookies =
            parse_cookie_input("MUSIC_U00AB12CD34EF56AB78CD90EF12AB34CD.music.163.com/2027-09-01");
        assert_eq!(
            cookies.get("MUSIC_U").map(String::as_str),
            Some("00AB12CD34EF56AB78CD90EF12AB34CD")
        );
        assert_eq!(cookies.len(), 1);

        // 域名没有前导点（host-only Cookie）时同样要截断。
        let cookies = parse_cookie_input("MUSIC_U00AB12CD34EF56AB78CD90EF12AB34CDmusic.163.com/");
        assert_eq!(
            cookies.get("MUSIC_U").map(String::as_str),
            Some("00AB12CD34EF56AB78CD90EF12AB34CD")
        );
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

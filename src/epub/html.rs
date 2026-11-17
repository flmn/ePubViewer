//! 字节级 HTML 扫描：找标签、读属性、清理章节正文。
//!
//! 章节正文原样交给 WebView 渲染，这里只提供拼装时需要的少量读写操作，
//! 不引入完整的 DOM 解析。

/// 大小写不敏感地找 `needle`（ASCII 标签名），从 `from` 起。
pub(super) fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || from > h.len() || h.len() < n.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| {
        h[i..i + n.len()]
            .iter()
            .zip(n)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

pub(super) fn find_byte(hay: &str, byte: u8, from: usize) -> Option<usize> {
    let start = from.min(hay.len());
    hay.as_bytes()[start..]
        .iter()
        .position(|b| *b == byte)
        .map(|i| i + start)
}

/// 标签名，小写；闭合标签带前导 `/`（`</OL>` → `/ol`）。
pub(super) fn tag_name(tag: &str) -> String {
    let mut rest = tag.trim_start_matches('<');
    let closing = rest.starts_with('/');
    if closing {
        rest = &rest[1..];
    }
    let mut name = String::new();
    for c in rest.chars() {
        if c.is_ascii_whitespace() || c == '>' || c == '/' {
            break;
        }
        name.push(c.to_ascii_lowercase());
    }
    if closing {
        format!("/{name}")
    } else {
        name
    }
}

/// 属性值的起止位置（相对 `tag`）。
pub(super) fn attr_range(tag: &str, name: &str) -> Option<(usize, usize)> {
    attr_bounds(tag, name).map(|(_, _, from, to)| (from, to))
}

/// 整个属性的范围（名字起点到值结尾，带引号时含引号），用于整段替换。
pub(super) fn attr_whole(tag: &str, name: &str) -> Option<(usize, usize)> {
    let (name_start, _, value_start, value_end) = attr_bounds(tag, name)?;
    let quoted = value_start > 0 && matches!(tag.as_bytes()[value_start - 1], b'"' | b'\'');
    Some((name_start, if quoted { value_end + 1 } else { value_end }))
}

/// 属性名与值的位置：名字起点、名字终点、值起点、值终点（不含引号）。
fn attr_bounds(tag: &str, name: &str) -> Option<(usize, usize, usize, usize)> {
    let bytes = tag.as_bytes();
    let needle = name.as_bytes();
    let mut i = 1;
    while i + needle.len() < bytes.len() {
        if bytes[i - 1].is_ascii_whitespace()
            && bytes[i..i + needle.len()]
                .iter()
                .zip(needle)
                .all(|(a, b)| a.eq_ignore_ascii_case(b))
            && bytes.get(i + needle.len()) == Some(&b'=')
        {
            let name_start = i;
            let name_end = i + needle.len();
            let mut j = name_end + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let quote = *bytes.get(j)?;
            if quote == b'"' || quote == b'\'' {
                let from = j + 1;
                let to = tag[from..].find(quote as char)? + from;
                return Some((name_start, name_end, from, to));
            }
            let mut to = j;
            while to < bytes.len() && !bytes[to].is_ascii_whitespace() && bytes[to] != b'>' {
                to += 1;
            }
            return Some((name_start, name_end, j, to));
        }
        i += 1;
    }
    None
}

/// 取 `<body>` 内容，并保留 body 上的 class（书籍 CSS 常把版式挂在 body 上）。
pub(super) fn extract_body(s: &str) -> (String, String) {
    if let Some(start) = find_ci(s, "<body", 0) {
        if let Some(gt) = find_byte(s, b'>', start) {
            let end = find_ci(s, "</body", gt).unwrap_or(s.len());
            let class = attr_range(&s[start..gt + 1], "class")
                .map(|(a, b)| s[start + a..start + b].to_owned())
                .unwrap_or_default();
            return (s[gt + 1..end].to_owned(), class);
        }
    }
    (s.to_owned(), String::new())
}

/// 去掉脚本、外链样式等，正文里的 `<style>` 保留。
pub(super) fn strip_disallowed(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while let Some(start) = find_byte(s, b'<', pos) {
        let Some(gt) = find_byte(s, b'>', start) else { break };
        out.push_str(&s[pos..start]);
        let tag = &s[start..gt + 1];
        let name = tag_name(tag);
        let bare = name.trim_start_matches('/');
        let paired = matches!(bare, "script" | "iframe" | "object");
        if matches!(bare, "script" | "iframe" | "object" | "link" | "meta" | "base") {
            if paired {
                let closing = format!("</{bare}");
                if let Some(end) = find_ci(s, &closing, gt) {
                    pos = find_byte(s, b'>', end).map_or(s.len(), |i| i + 1);
                    continue;
                }
            }
            pos = gt + 1;
            continue;
        }
        out.push_str(tag);
        pos = gt + 1;
    }
    out.push_str(&s[pos..]);
    out
}

/// 有些工具（如 Calibre）会把作者写的 `&nbsp;` 再转义成 `&amp;nbsp;`，
/// 浏览器会把它当字面量显示。这里把常见命名实体还原成单次转义。
pub(super) fn fix_double_escaped_entities(s: &str) -> String {
    const NAMES: &[&str] = &[
        "nbsp", "ensp", "emsp", "thinsp", "mdash", "ndash", "hellip", "ldquo", "rdquo", "lsquo",
        "rsquo", "middot", "bull", "copy", "reg", "trade", "times", "divide", "deg", "laquo",
        "raquo", "sect", "para", "dagger", "euro", "pound", "yen", "frac12", "frac14", "frac34",
        "sup2", "sup3", "plusmn", "ne", "le", "ge",
    ];
    if !s.contains("&amp;") {
        return s.to_owned();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("&amp;") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 5..];
        let mut hit = None;
        for name in NAMES {
            if let Some(after) = tail.strip_prefix(name).and_then(|t| t.strip_prefix(';')) {
                hit = Some((*name, after));
                break;
            }
        }
        match hit {
            Some((name, after)) => {
                out.push('&');
                out.push_str(name);
                out.push(';');
                rest = after;
            }
            None => {
                out.push_str("&amp;");
                rest = tail;
            }
        }
    }
    out.push_str(rest);
    out
}

/// 标签之间的纯文本，压缩空白并解出常见实体；目录标签用。
pub(super) fn plain_text(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(super) fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

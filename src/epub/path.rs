//! EPUB 里的路径：相对地址解析、百分号解码、`epub://` 反解与 MIME 推断。

use base64::{Engine as _, engine::general_purpose::STANDARD};

/// `epub://OEBPS/images/a.png` → `OEBPS/images/a.png`。
///
/// 文档里的资源地址由本模块写进 `epub://`，宿主再按同一个规则反解，
/// 两边必须保持一致（去掉 scheme 与前导 `/`，再做一次百分号解码）。
pub(super) fn scheme_path(url: &str) -> String {
    let rest = url
        .strip_prefix("epub://")
        .or_else(|| url.strip_prefix("epub:"))
        .unwrap_or(url);
    percent_decode(rest.trim_start_matches('/'))
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// ZIP 内路径的目录部分，`OEBPS/text/ch1.xhtml` → `OEBPS/text`。
pub(super) fn dir_of(path: &str) -> String {
    path.rfind('/').map_or_else(String::new, |i| path[..i].to_owned())
}

/// 把 `href` 解析成 ZIP 内的相对路径，处理 `./`、`../` 与根路径。
pub(super) fn resolve(base: &str, href: &str) -> String {
    let href = href.trim_start_matches("./");
    if href.starts_with('/') {
        return href.trim_start_matches('/').to_owned();
    }
    let mut parts: Vec<&str> = base
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    for part in href.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

/// 按扩展名推断 MIME，未知类型退回 `application/octet-stream`。
pub(super) fn mime_of(path: &str) -> &'static str {
    let path = path.to_ascii_lowercase();
    let ext = path.rsplit('.').next().unwrap_or("");
    match ext {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "css" => "text/css",
        "xhtml" | "html" | "htm" => "application/xhtml+xml",
        "js" => "text/javascript",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "ogg" => "audio/ogg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// 字节的 `data:` URL，供内联字体、背景图与宿主回填的图片使用。
pub(super) fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

/// 这些地址不指向 ZIP 内的文件，改写与内联都要放过它们。
pub(super) fn is_remote(s: &str) -> bool {
    let s = s.trim();
    s.is_empty()
        || s.starts_with('#')
        || s.starts_with("data:")
        || s.starts_with("http:")
        || s.starts_with("https:")
        || s.starts_with("//")
        || s.starts_with("epub:")
        || s.starts_with("mailto:")
        || s.starts_with("tel:")
}

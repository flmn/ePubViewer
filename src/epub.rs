//! 把一个 EPUB（ZIP）解析成 TSX 能直接渲染的数据。
//!
//! 严格 XML（container.xml / OPF / NCX）用 `roxmltree`；nav.xhtml 与章节正文
//! 是 HTML，按容错方式扫描，正文原样交给 WebView 渲染。

use std::io::{Cursor, Read, Seek};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use nsh_core::prelude::*;
use roxmltree::{Document, ParsingOptions};
use zip::ZipArchive;

/// 一本解析好的书。
#[derive(Debug, Clone, JsObject)]
pub struct Book {
    /// 书名。
    pub title: String,
    /// 按 spine 顺序排列的章节。
    pub chapters: Vec<Chapter>,
    /// 目录（EPUB3 nav 优先，否则 EPUB2 NCX）。
    pub toc: Vec<TocEntry>,
}

/// 一章的元信息；正文按需用 `Epub.chapter()` 取。
#[derive(Debug, Clone, JsObject)]
pub struct Chapter {
    /// 章节在 ZIP 内的路径，用来匹配目录链接。
    pub id: String,
    /// 章节标题。
    pub title: String,
}

/// 已打开的书：原始字节留在 Rust 侧，章节正文按需取出，
/// 避免把整本书（含图片）一次性塞给 WebView。
pub struct Loaded {
    pub book: Book,
    bytes: Vec<u8>,
    chapter_paths: Vec<String>,
}

/// 一个目录项。
#[derive(Debug, Clone, JsObject)]
pub struct TocEntry {
    /// 目录文字。
    pub label: String,
    /// 目标章节的下标。
    pub chapter: u32,
    /// 层级，0 为顶层。
    pub level: u32,
    /// 章节内的锚点，没有则为空串。
    pub anchor: String,
}

struct Item {
    id: String,
    href: String,
    media_type: String,
    properties: String,
}

struct RawToc {
    label: String,
    target: String,
    level: u32,
}

pub fn parse(data: Vec<u8>) -> Result<Loaded, String> {
    let saved = data.clone();
    let mut archive = ZipArchive::new(Cursor::new(data))
        .map_err(|e| format!("不是有效的 EPUB 文件: {e}"))?;
    let names: Vec<String> = archive
        .file_names()
        .filter_map(|name| name.ok())
        .map(|name| name.into_owned())
        .collect();

    let container = read(&mut archive, &names, "META-INF/container.xml")?;
    let container_xml = decode_xml(&container);
    let container = parse_xml(&container_xml)
        .map_err(|e| format!("container.xml 解析失败: {e}"))?;
    let opf_path = container
        .descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .map(percent_decode)
        .ok_or_else(|| "container.xml 缺少 rootfile".to_owned())?;
    let opf_dir = dir_of(&opf_path);

    let opf_bytes = read(&mut archive, &names, &opf_path)?;
    let opf_xml = decode_xml(&opf_bytes);
    let opf = parse_xml(&opf_xml)
        .map_err(|e| format!("OPF 解析失败: {e}"))?;

    let title = opf
        .descendants()
        .find(|n| n.tag_name().name() == "title")
        .and_then(|n| n.text())
        .unwrap_or("未命名")
        .trim()
        .to_owned();

    let mut items = Vec::new();
    for node in opf.descendants().filter(|n| n.tag_name().name() == "item") {
        let (Some(id), Some(href)) = (node.attribute("id"), node.attribute("href")) else {
            continue;
        };
        items.push(Item {
            id: id.to_owned(),
            href: resolve(&opf_dir, &percent_decode(href)),
            media_type: node.attribute("media-type").unwrap_or("").to_owned(),
            properties: node.attribute("properties").unwrap_or("").to_owned(),
        });
    }

    let spine: Vec<String> = opf
        .descendants()
        .filter(|n| n.tag_name().name() == "itemref")
        .filter(|n| n.attribute("linear") != Some("no"))
        .filter_map(|n| n.attribute("idref").map(str::to_owned))
        .collect();

    let mut chapters = Vec::new();
    for id in &spine {
        let Some(item) = items.iter().find(|i| &i.id == id) else {
            continue;
        };
        chapters.push(Chapter {
            id: item.href.clone(),
            title: format!("第 {} 节", chapters.len() + 1),
        });
    }
    if chapters.is_empty() {
        return Err("这个 EPUB 没有可阅读的章节".to_owned());
    }

    let nav = items
        .iter()
        .find(|i| i.properties.split_whitespace().any(|p| p == "nav"));
    let ncx = items
        .iter()
        .find(|i| i.media_type == "application/x-dtbncx+xml")
        .or_else(|| {
            opf.descendants()
                .find(|n| n.tag_name().name() == "spine")
                .and_then(|n| n.attribute("toc"))
                .and_then(|id| items.iter().find(|i| i.id == id))
        });

    let mut raw_toc = Vec::new();
    if let Some(item) = nav {
        let bytes = read(&mut archive, &names, &item.href).unwrap_or_default();
        raw_toc = parse_nav(&String::from_utf8_lossy(&bytes), &dir_of(&item.href));
    }
    if raw_toc.is_empty() {
        if let Some(item) = ncx {
            let bytes = read(&mut archive, &names, &item.href).unwrap_or_default();
            raw_toc = parse_ncx(&String::from_utf8_lossy(&bytes), &dir_of(&item.href));
        }
    }

    let toc: Vec<TocEntry> = raw_toc
        .iter()
        .map(|t| {
            let (chapter, anchor) = locate(&t.target, &chapters);
            TocEntry {
                label: t.label.clone(),
                chapter,
                level: t.level,
                anchor,
            }
        })
        .collect();

    for (i, chapter) in chapters.iter_mut().enumerate() {
        if let Some(entry) = toc.iter().find(|t| t.chapter as usize == i) {
            chapter.title = entry.label.clone();
        }
    }

    let book = Book { title, chapters, toc };
    let chapter_paths = book.chapters.iter().map(|c| c.id.clone()).collect();
    Ok(Loaded {
        book,
        bytes: saved,
        chapter_paths,
    })
}

impl Loaded {
    /// 生成整本书的单文档 HTML：所有章节按顺序拼成一个多列文档。
    /// 图片、音视频写成 `epub://` 占位等宿主回填（`resource()`），样式表里
    /// 的字体和背景图则在拼装时直接内联成 `data:` URL。
    pub fn book_html(&self) -> String {
        let Ok(mut archive) = ZipArchive::new(Cursor::new(self.bytes.as_slice())) else {
            return String::new();
        };
        let names: Vec<String> = archive
            .file_names()
            .filter_map(|name| name.ok())
            .map(|name| name.into_owned())
            .collect();
        let css = self.book_css(&mut archive, &names);
        let mut content = String::new();
        for (i, path) in self.chapter_paths.iter().enumerate() {
            let Ok(bytes) = read(&mut archive, &names, path) else {
                continue;
            };
            let raw = decode_xml(&bytes);
            let (body, class) = extract_body(&raw);
            let body = fix_double_escaped_entities(&body);
            let body = strip_disallowed(&body);
            let body = prefix_ids(&body, i);
            let body = rewrite_links(&body, i, &dir_of(path), &self.chapter_paths);
            let body = rewrite_assets(&body, &dir_of(path));
            let class = if class.is_empty() {
                String::new()
            } else {
                format!(" {}", escape_html(&class))
            };
            content.push_str(&format!(
                "<section id=\"ch{i}\" data-chapter=\"{i}\" class=\"epub-chapter{class}\">{body}</section>\n"
            ));
        }
        book_document(&self.book.title, &css, &content)
    }

    /// 把文档里的 `epub://OEBPS/images/a.png` 变成可以直接塞进 `src` 的
    /// `data:` URL；ZIP 里没有这个条目时返回 `None`。
    pub fn resource(&self, url: &str) -> Option<String> {
        let path = scheme_path(url);
        if path.is_empty() {
            return None;
        }
        let mut archive = ZipArchive::new(Cursor::new(self.bytes.as_slice())).ok()?;
        let names: Vec<String> = archive
            .file_names()
            .filter_map(|name| name.ok())
            .map(|name| name.into_owned())
            .collect();
        let bytes = read(&mut archive, &names, &path).ok()?;
        Some(data_url(mime_of(&path), &bytes))
    }

    /// 收集全书样式：各章 `<link rel="stylesheet">` 指向的文件（去重）
    /// 与 `<style>` 块。CSS 里的 `url(...)`（字体、背景图）没法等宿主
    /// 回填，直接内联成 `data:` URL。
    fn book_css<R: Read + Seek>(&self, archive: &mut ZipArchive<R>, names: &[String]) -> String {
        let mut seen = std::collections::HashSet::new();
        let mut out = String::new();
        for path in &self.chapter_paths {
            let Ok(bytes) = read(archive, names, path) else {
                continue;
            };
            let raw = decode_xml(&bytes);
            let base = dir_of(path);
            let mut pos = 0;
            while let Some(start) = find_ci(&raw, "<link", pos) {
                let Some(gt) = find_byte(&raw, b'>', start) else { break };
                let tag = &raw[start..gt + 1];
                let rel = attr_range(tag, "rel")
                    .map(|(a, b)| tag[a..b].to_ascii_lowercase())
                    .unwrap_or_default();
                if rel.split_whitespace().any(|t| t == "stylesheet") {
                    if let Some((a, b)) = attr_range(tag, "href") {
                        let css_path = resolve(&base, &percent_decode(&tag[a..b]));
                        if seen.insert(css_path.clone()) {
                            if let Ok(css) = read(archive, names, &css_path) {
                                let css_dir = dir_of(&css_path);
                                out.push_str(&format!("/* {css_path} */\n"));
                                let text = inline_css_urls(
                                    &String::from_utf8_lossy(&css),
                                    &css_dir,
                                    |path| read(archive, names, path).ok(),
                                );
                                out.push_str(&text);
                                out.push('\n');
                            }
                        }
                    }
                }
                pos = gt + 1;
            }
            let mut pos = 0;
            while let Some(start) = find_ci(&raw, "<style", pos) {
                let Some(gt) = find_byte(&raw, b'>', start) else { break };
                let close = find_ci(&raw, "</style", gt).unwrap_or(raw.len());
                let css = &raw[gt + 1..close.min(raw.len())];
                out.push_str(&inline_css_urls(css, &base, |path| {
                    read(archive, names, path).ok()
                }));
                out.push('\n');
                pos = find_byte(&raw, b'>', close).map_or(raw.len(), |i| i + 1);
            }
        }
        out
    }
}

/// 有些工具（如 Calibre）会把作者写的 `&nbsp;` 再转义成 `&amp;nbsp;`，
/// 浏览器会把它当字面量显示。这里把常见命名实体还原成单次转义。
fn fix_double_escaped_entities(s: &str) -> String {
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

/// 取 `<body>` 内容，并保留 body 上的 class（书籍 CSS 常把版式挂在 body 上）。
fn extract_body(s: &str) -> (String, String) {
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
fn strip_disallowed(s: &str) -> String {
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

/// EPUB3 `nav.xhtml`：取第一个 `<nav>`，按 `<ol>/<ul>` 深度记录层级。
fn parse_nav(html: &str, base: &str) -> Vec<RawToc> {
    let start = find_ci(html, "<nav", 0).unwrap_or(0);
    let open_end = find_byte(html, b'>', start).unwrap_or(html.len());
    let end = find_ci(html, "</nav", open_end).unwrap_or(html.len());
    let section = &html[(open_end + 1).min(end)..end];

    let mut out = Vec::new();
    let mut level = 0i32;
    let mut pos = 0;
    while let Some(start) = find_byte(section, b'<', pos) {
        let Some(gt) = find_byte(section, b'>', start) else { break };
        let tag = &section[start..gt + 1];
        match tag_name(tag).as_str() {
            "ol" | "ul" => level += 1,
            "/ol" | "/ul" => level = (level - 1).max(0),
            "a" => {
                let close = find_ci(section, "</a", gt).unwrap_or(gt + 1).max(gt + 1);
                let label = plain_text(&section[gt + 1..close]);
                let href = attr_range(tag, "href")
                    .map(|(a, b)| &tag[a..b])
                    .unwrap_or("");
                if !label.is_empty() {
                    out.push(RawToc {
                        label,
                        target: resolve_target(base, href),
                        level: (level - 1).max(0) as u32,
                    });
                }
                pos = close;
                continue;
            }
            _ => {}
        }
        pos = gt + 1;
    }
    out
}

/// EPUB2 NCX：`navPoint` 的嵌套深度就是目录层级。
fn parse_ncx(xml: &str, base: &str) -> Vec<RawToc> {
    let xml = xml
        .replace("&nbsp;", "&#160;")
        .replace("&mdash;", "&#8212;")
        .replace("&ndash;", "&#8211;")
        .replace("&hellip;", "&#8230;");
    let Ok(doc) = parse_xml(&xml) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for node in doc.descendants().filter(|n| n.tag_name().name() == "navPoint") {
        let level = node
            .ancestors()
            .filter(|n| n.tag_name().name() == "navPoint")
            .count()
            .saturating_sub(1) as u32;
        let label = node
            .children()
            .find(|n| n.tag_name().name() == "navLabel")
            .and_then(|n| n.descendants().find(|d| d.tag_name().name() == "text"))
            .and_then(|n| n.text())
            .unwrap_or("")
            .trim()
            .to_owned();
        let src = node
            .children()
            .find(|n| n.tag_name().name() == "content")
            .and_then(|n| n.attribute("src"))
            .unwrap_or("");
        if label.is_empty() {
            continue;
        }
        out.push(RawToc {
            label,
            target: resolve_target(base, src),
            level,
        });
    }
    out
}

fn resolve_target(base: &str, href: &str) -> String {
    let (file, anchor) = href.split_once('#').unwrap_or((href, ""));
    let decoded = percent_decode(file);
    if decoded.is_empty() {
        return format!("#{anchor}");
    }
    let path = resolve(base, &decoded);
    if anchor.is_empty() {
        path
    } else {
        format!("{path}#{anchor}")
    }
}

fn locate(target: &str, chapters: &[Chapter]) -> (u32, String) {
    let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
    if file.is_empty() {
        return (0, anchor.to_owned());
    }
    if let Some(i) = chapters.iter().position(|c| c.id == file) {
        return (i as u32, anchor.to_owned());
    }
    let tail = file.rsplit('/').next().unwrap_or(file);
    if let Some(i) = chapters
        .iter()
        .position(|c| c.id.rsplit('/').next() == Some(tail))
    {
        return (i as u32, anchor.to_owned());
    }
    (0, anchor.to_owned())
}

fn read<R: Read + Seek>(archive: &mut ZipArchive<R>, names: &[String], path: &str) -> Result<Vec<u8>, String> {
    let name = match_name(names, path);
    let mut file = archive
        .by_name(&name)
        .map_err(|e| format!("EPUB 内找不到 {path}: {e}"))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("读取 {path} 失败: {e}"))?;
    Ok(buf)
}

fn match_name(names: &[String], path: &str) -> String {
    if names.iter().any(|n| n == path) {
        return path.to_owned();
    }
    names
        .iter()
        .find(|n| n.eq_ignore_ascii_case(path) || n.ends_with(path) || path.ends_with(n.as_str()))
        .cloned()
        .unwrap_or_else(|| path.to_owned())
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

fn dir_of(path: &str) -> String {
    path.rfind('/').map_or_else(String::new, |i| path[..i].to_owned())
}

/// 解析严格 XML。EPUB2 的 NCX 几乎都带 DOCTYPE，roxmltree 默认拒绝 DTD，
/// 这里显式打开 `allow_dtd`；外部 DTD 不会加载，实体仍需提前替换成数字引用。
fn parse_xml(xml: &str) -> Result<Document<'_>, roxmltree::Error> {
    Document::parse_with_options(
        xml,
        ParsingOptions {
            allow_dtd: true,
            ..ParsingOptions::default()
        },
    )
}

/// 优先按 XML 声明的编码解码（EPUB 里偶见 GBK 等），否则按 UTF-8。
fn decode_xml(bytes: &[u8]) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(256)]);
    let enc = head
        .split("encoding=\"")
        .nth(1)
        .or_else(|| head.split("encoding='").nth(1))
        .and_then(|rest| rest.split(['"', '\'']).next())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !enc.is_empty() && enc != "utf-8" && enc != "utf8" {
        if let Some(encoding) = encoding_rs::Encoding::for_label(enc.as_bytes()) {
            let (text, _, had_errors) = encoding.decode(bytes);
            if !had_errors {
                return text.into_owned();
            }
        }
    }
    String::from_utf8_lossy(bytes).into_owned()
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn resolve(base: &str, href: &str) -> String {
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

fn mime_of(path: &str) -> &'static str {
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

fn tag_name(tag: &str) -> String {
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
fn attr_range(tag: &str, name: &str) -> Option<(usize, usize)> {
    attr_bounds(tag, name).map(|(_, _, from, to)| (from, to))
}

/// 整个属性的范围（名字起点到值结尾，带引号时含引号）。
fn attr_whole(tag: &str, name: &str) -> Option<(usize, usize)> {
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

fn plain_text(s: &str) -> String {
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

/// 大小写不敏感地找 `needle`（ASCII 标签名），从 `from` 起。
fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
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

fn find_byte(hay: &str, byte: u8, from: usize) -> Option<usize> {
    let start = from.min(hay.len());
    hay.as_bytes()[start..]
        .iter()
        .position(|b| *b == byte)
        .map(|i| i + start)
}

const BOOK_HEAD: &str =
    "<!doctype html><html><head><meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>";

const BOOK_STYLE: &str = r#"</title><style>
  html,body { margin:0; padding:0; height:100%; overflow:hidden;
    background:#fffdf8; color:#2c2a26;
    font-family:-apple-system,"PingFang SC","Songti SC",serif;
    font-size:18px; line-height:1.85; }
  #viewport { position:fixed; inset:44px 56px; overflow:hidden; }
  #content { position:relative; height:100%; column-gap:0; column-fill:auto;
    transform:translateX(0); }
  .epub-chapter { display:block; }
  /* 兜底文本样式整体放进 :where()（0 特异性），书籍的 .calibreN 等类规则可直接覆盖 */
  :where(#content p) { margin:0 0 1em; text-align:justify; }
  :where(#content h1,#content h2,#content h3,#content h4,#content h5,#content h6) { line-height:1.4; margin:1.2em 0 .6em; }
  /* 图片整体落在同一页：限制在一页高度内，且不跨列断开 */
  #content img { max-width:100%; max-height:var(--page-height,100%); height:auto;
    object-fit:contain; break-inside:avoid; }
  :where(#content figure,#content table,#content pre) { break-inside:avoid; }
  :where(#content a) { color:#8a6a3a; text-decoration:none; }
  :where(#content blockquote) { margin:1em 0; padding-left:1em; border-left:3px solid #ddd8cc; }
  :where(#content pre) { white-space:pre-wrap; }
</style><style>"#;

const BOOK_BODY: &str = "</style></head><body>\n<div id=\"viewport\"><div id=\"content\">";

const BOOK_SCRIPT: &str = r#"</div></div>
<script>
(function(){
  var viewport=document.getElementById('viewport');
  var content=document.getElementById('content');
  var sections=content.querySelectorAll('section.epub-chapter');
  var width=1,total=1,current=0;
  var lastPost='';
  var resEls={};
  function post(msg){
    if(!window.nsh||!window.nsh.postMessage) return;
    var s=JSON.stringify(msg);
    if(s===lastPost) return;
    lastPost=s;
    window.nsh.postMessage(s);
  }
  function chapterAt(x){
    var best=0;
    for(var i=0;i<sections.length;i++){
      if(sections[i].offsetLeft<=x+1) best=i; else break;
    }
    return best;
  }
  function setPage(n,report){
    current=Math.max(0,Math.min(n,total-1));
    content.style.transform='translateX('+(-current*width)+'px)';
    if(report) post({type:'page',page:current,total:total,chapter:chapterAt(current*width)});
  }
  function layout(){
    width=Math.max(1,viewport.clientWidth);
    content.style.width=width+'px';
    content.style.columnWidth=width+'px';
    content.style.height=viewport.clientHeight+'px';
    content.style.setProperty('--page-height',viewport.clientHeight+'px');
    total=Math.max(1,Math.ceil((content.scrollWidth-1)/width));
    if(current>total-1) current=total-1;
    setPage(current,false);
    post({type:'ready',total:total,page:current,chapter:chapterAt(current*width)});
  }
  /* 元素落在第几页：列的排布不随 translateX 变化，换算回绝对位置 */
  function pageOf(el){
    var left=el.getBoundingClientRect().left-content.getBoundingClientRect().left+current*width;
    return Math.max(0,Math.floor(left/width));
  }
  function goAnchor(id){
    var el=id?document.getElementById(id):null;
    if(!el){ setPage(0,true); return; }
    setPage(pageOf(el),true);
  }
  window.__epubSetPage=function(n){ setPage(n,true); };
  window.__epubLayout=layout;
  window.__epubGoToAnchor=goAnchor;
  window.__epubGoToChapter=function(i){
    var el=sections[i];
    if(!el) return;
    setPage(pageOf(el),true);
  };
  /* 宿主把 epub:// 换成 data: URL 后填回原属性 */
  window.__epubRes=function(id,data){
    var el=resEls[id];
    if(!el||!data) return;
    var attr=el.getAttribute('data-epub-attr')||'src';
    el.removeAttribute('data-epub');
    el.removeAttribute('data-epub-attr');
    try{ el.setAttribute(attr,data); }catch(e){ return; }
    if(attr!=='src'){
      try{ el.setAttributeNS('http://www.w3.org/1999/xlink','xlink:href',data); }catch(e){}
    }
  };
  document.addEventListener('click',function(e){
    var a=e.target&&e.target.closest?e.target.closest('a'):null;
    if(!a) return;
    var href=a.getAttribute('href')||'';
    e.preventDefault();
    if(href.charAt(0)==='#') goAnchor(href.slice(1));
    else post({type:'link',href:href});
  });
  content.addEventListener('load',function(){ layout(); },true);
  window.addEventListener('resize',layout);
  window.addEventListener('load',function(){ setTimeout(layout,0); });
  setTimeout(layout,0);
  /* 把 epub:// 占位交回宿主换取字节；宿主再调用 __epubRes 填回 */
  var requested=false;
  function requestAssets(force){
    if(requested && !force) return;
    requested=true;
    var items=[],assets=content.querySelectorAll('[data-epub]');
    for(var i=0;i<assets.length;i++){
      var id='r'+i;
      resEls[id]=assets[i];
      items.push({id:id,url:assets[i].getAttribute('data-epub')});
    }
    if(items.length) post({type:'res',items:items});
  }
  window.__epubRequestAssets=requestAssets;
  setTimeout(requestAssets,0);
})();
</script>"#;

/// 拼出整本书的 HTML 文档。
fn book_document(title: &str, css: &str, content: &str) -> String {
    let mut out = String::with_capacity(content.len() + css.len() + 8192);
    out.push_str(BOOK_HEAD);
    out.push_str(&escape_html(title));
    out.push_str(BOOK_STYLE);
    out.push_str(css);
    out.push_str(BOOK_BODY);
    out.push_str(content);
    out.push_str(BOOK_SCRIPT);
    out.push_str("</body></html>");
    out
}

/// 给章内 `id` / `name` 加章节前缀，避免不同章节的同名锚点互相覆盖；
/// `href="#x"` 同步改成 `href="#chN-x"`。
fn prefix_ids(body: &str, chapter: usize) -> String {
    let prefix = format!("ch{chapter}-");
    let mut out = String::with_capacity(body.len());
    let mut pos = 0;
    while let Some(start) = find_byte(body, b'<', pos) {
        let Some(gt) = find_byte(body, b'>', start) else { break };
        out.push_str(&body[pos..start]);
        let mut tag = body[start..gt + 1].to_owned();
        for attr in ["id", "name"] {
            if let Some((a, b)) = attr_range(&tag, attr) {
                let value = tag[a..b].to_owned();
                if !value.is_empty() && !value.starts_with(&prefix) {
                    tag.replace_range(a..b, &format!("{prefix}{value}"));
                }
            }
        }
        if let Some((a, b)) = attr_range(&tag, "href") {
            let value = tag[a..b].to_owned();
            if let Some(anchor) = value.strip_prefix('#') {
                if !anchor.starts_with(&prefix) {
                    tag.replace_range(a..b, &format!("#{prefix}{anchor}"));
                }
            }
        }
        out.push_str(&tag);
        pos = gt + 1;
    }
    out.push_str(&body[pos..]);
    out
}

/// 跨章链接（`ch2.xhtml#sec`）改成本文档里的锚点（`#ch1-sec`）。
/// 站外链接与已经前缀过的章内锚点原样留下。
fn rewrite_links(s: &str, chapter: usize, base: &str, paths: &[String]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while let Some(start) = find_byte(s, b'<', pos) {
        let Some(gt) = find_byte(s, b'>', start) else { break };
        out.push_str(&s[pos..start]);
        let mut tag = s[start..gt + 1].to_owned();
        if tag_name(&tag) == "a"
            && let Some((a, b)) = attr_range(&tag, "href")
            && let Some(target) = local_anchor(&tag[a..b], chapter, base, paths)
        {
            tag.replace_range(a..b, &target);
        }
        out.push_str(&tag);
        pos = gt + 1;
    }
    out.push_str(&s[pos..]);
    out
}

fn local_anchor(href: &str, chapter: usize, base: &str, paths: &[String]) -> Option<String> {
    if is_remote(href) || href.starts_with('#') {
        return None;
    }
    let (file, anchor) = href.split_once('#').unwrap_or((href, ""));
    let index = if file.is_empty() {
        chapter
    } else {
        chapter_of(&resolve(base, &percent_decode(file)), paths)?
    };
    Some(if anchor.is_empty() {
        format!("#ch{index}")
    } else {
        format!("#ch{index}-{anchor}")
    })
}

/// EPUB 里的路径与 spine 顺序对上；找不到时退回按文件名匹配。
fn chapter_of(path: &str, paths: &[String]) -> Option<usize> {
    if let Some(i) = paths.iter().position(|p| p == path) {
        return Some(i);
    }
    let tail = path.rsplit('/').next()?;
    paths
        .iter()
        .position(|p| p.rsplit('/').next() == Some(tail))
}

/// 图片、SVG、音视频的地址改写成 `epub://`，由应用按需提供字节。
///
/// 原来的属性名换成 `data-epub` / `data-epub-attr`：WebView 不认识
/// `epub://`，留着 `src` 只会先失败一次；页面脚本拿到字节后再填回去。
fn rewrite_assets(s: &str, base: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while let Some(start) = find_byte(s, b'<', pos) {
        let Some(gt) = find_byte(s, b'>', start) else { break };
        out.push_str(&s[pos..start]);
        let mut tag = s[start..gt + 1].to_owned();
        if matches!(
            tag_name(&tag).as_str(),
            "img" | "image" | "video" | "audio" | "source"
        ) {
            for attr in ["src", "xlink:href", "href", "poster"] {
                let Some((a, b)) = attr_range(&tag, attr) else { continue };
                let value = tag[a..b].to_owned();
                if is_remote(&value) {
                    continue;
                }
                let Some((from, to)) = attr_whole(&tag, attr) else { continue };
                let uri = format!("epub://{}", resolve(base, &percent_decode(&value)));
                tag.replace_range(
                    from..to,
                    &format!("data-epub=\"{uri}\" data-epub-attr=\"{attr}\""),
                );
            }
        }
        out.push_str(&tag);
        pos = gt + 1;
    }
    out.push_str(&s[pos..]);
    out
}

/// 样式表里 `url(...)` 的相对地址内联成 `data:` URL（字体、背景图）。
fn inline_css_urls(
    css: &str,
    base: &str,
    mut lookup: impl FnMut(&str) -> Option<Vec<u8>>,
) -> String {
    let mut out = String::with_capacity(css.len());
    let mut pos = 0;
    while let Some(start) = find_ci(css, "url(", pos) {
        out.push_str(&css[pos..start]);
        let Some(end) = find_byte(css, b')', start) else { break };
        let raw = css[start + 4..end].trim().trim_matches(['"', '\'']).trim();
        let path = resolve(base, &percent_decode(raw));
        match (!is_remote(raw)).then(|| lookup(&path)).flatten() {
            Some(bytes) => {
                out.push_str("url(\"");
                out.push_str(&data_url(mime_of(&path), &bytes));
                out.push_str("\")");
            }
            None => out.push_str(&css[start..end + 1]),
        }
        pos = end + 1;
    }
    out.push_str(&css[pos..]);
    out
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", STANDARD.encode(bytes))
}

fn is_remote(s: &str) -> bool {
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

/// `epub://OEBPS/images/a.png` → `OEBPS/images/a.png`。
pub fn scheme_path(url: &str) -> String {
    let rest = url
        .strip_prefix("epub://")
        .or_else(|| url.strip_prefix("epub:"))
        .unwrap_or(url);
    percent_decode(rest.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::CompressionMethod;
    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    fn add(writer: &mut ZipWriter<Cursor<Vec<u8>>>, name: &str, body: &[u8]) {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file(name, options).unwrap();
        writer.write_all(body).unwrap();
    }

    fn sample(epub3: bool) -> Vec<u8> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        add(
            &mut w,
            "META-INF/container.xml",
            r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#.as_bytes(),
        );
        let nav_item = if epub3 {
            r#"<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>"#
        } else {
            ""
        };
        let opf = format!(
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>测试书</dc:title></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/><item id="ch2" href="ch2.xhtml" media-type="application/xhtml+xml"/><item id="img" href="images/cover.png" media-type="image/png"/>{nav_item}</manifest><spine toc="ncx"><itemref idref="ch1"/><itemref idref="ch2"/></spine></package>"#
        );
        add(&mut w, "OEBPS/content.opf", opf.as_bytes());
        add(
            &mut w,
            "OEBPS/toc.ncx",
            r#"<?xml version="1.0"?><!DOCTYPE ncx PUBLIC "-//NISO//DTD ncx 2005-1//EN" "http://www.daisy.org/z3986/2005/ncx-2005-1.dtd"><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint id="n1"><navLabel><text>第一章</text></navLabel><content src="ch1.xhtml"/></navPoint><navPoint id="n2"><navLabel><text>第二章</text></navLabel><content src="ch2.xhtml"/><navPoint id="n3"><navLabel><text>小节</text></navLabel><content src="ch2.xhtml#sec2"/></navPoint></navPoint></navMap></ncx>"#.as_bytes(),
        );
        if epub3 {
            add(
                &mut w,
                "OEBPS/nav.xhtml",
                r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><body><nav epub:type="toc"><ol><li><a href="ch1.xhtml">第一章</a></li><li><a href="ch2.xhtml">第二章</a><ol><li><a href="ch2.xhtml#sec2">小节</a></li></ol></li></ol></nav></body></html>"#.as_bytes(),
            );
        }
        add(
            &mut w,
            "OEBPS/style.css",
            br#"p { margin: 0 0 .4em; } .lead { font-weight: bold; } .cover { background: url("images/cover.png"); }"#,
        );
        add(
            &mut w,
            "OEBPS/ch1.xhtml",
            r#"<html><head><link rel="stylesheet" href="style.css"/><style>.note { color: #666; }</style></head><body class="calibre2"><h1 id="top">第一章</h1><p class="lead">你好，世界。&amp;nbsp;测试</p><a href="ch2.xhtml#sec2">跳到第二章</a><img src="images/cover.png" alt="封面"/><script>bad()</script></body></html>"#.as_bytes(),
        );
        add(
            &mut w,
            "OEBPS/ch2.xhtml",
            r#"<html><body><p id="sec2">第二章正文</p></body></html>"#.as_bytes(),
        );
        add(&mut w, "OEBPS/images/cover.png", &[0x89, b'P', b'N', b'G']);
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn parses_epub2_ncx() {
        let book = parse(sample(false)).unwrap().book;
        assert_eq!(book.title, "测试书");
        assert_eq!(book.chapters.len(), 2);
        assert_eq!(book.toc.len(), 3);
        assert_eq!(book.toc[0].chapter, 0);
        assert_eq!(book.toc[1].chapter, 1);
        assert_eq!(book.toc[2].chapter, 1);
        assert_eq!(book.toc[2].anchor, "sec2");
        assert_eq!(book.toc[2].level, 1);
        assert_eq!(book.chapters[0].title, "第一章");
        assert_eq!(book.chapters[1].title, "第二章");
    }

    #[test]
    fn parses_epub3_nav() {
        let book = parse(sample(true)).unwrap().book;
        assert_eq!(book.toc.len(), 3);
        assert_eq!(book.toc[2].anchor, "sec2");
        assert_eq!(book.toc[2].level, 1);
    }

    #[test]
    fn book_html_merges_chapters_and_rewrites_assets() {
        let loaded = parse(sample(false)).unwrap();
        let html = loaded.book_html();
        assert!(html.contains("<!doctype html>"));
        // 两章都进了同一个文档，各自是一个 section。
        assert!(html.contains("id=\"ch0\""));
        assert!(html.contains("id=\"ch1\""));
        assert!(html.contains("第一章"));
        assert!(html.contains("第二章正文"));
        // &amp;nbsp; 还原成 &nbsp;，交给浏览器解码为空格。
        assert!(html.contains("你好，世界。&nbsp;测试"));
        assert!(!html.contains("&amp;nbsp;"));
        // 脚本、外链样式表不留在正文里。
        assert!(!html.contains("bad()"));
        assert!(!html.contains("<link"));
        // 图片改成 epub:// 占位，等宿主换成 data: URL。
        assert!(html.contains("data-epub=\"epub://OEBPS/images/cover.png\""));
        assert!(html.contains("data-epub-attr=\"src\""));
        // 外链样式表与内联 <style> 都被收集；CSS 里的 url() 直接内联。
        assert!(html.contains("/* OEBPS/style.css */"));
        assert!(html.contains("font-weight: bold"));
        assert!(html.contains(".note { color: #666; }"));
        assert!(html.contains("url(\"data:image/png;base64,"));
        // body 上的 class 保留到内容容器，书籍版式规则才能命中。
        assert!(html.contains("class=\"epub-chapter calibre2\""));
        // 锚点按章加前缀，跨章链接指向文档内的锚点。
        assert!(html.contains("id=\"ch0-top\""));
        assert!(html.contains("href=\"#ch1-sec2\""));
        assert!(html.contains("id=\"ch1-sec2\""));
    }

    #[test]
    fn resources_resolve_from_the_scheme() {
        let loaded = parse(sample(false)).unwrap();
        assert_eq!(scheme_path("epub://OEBPS/a%20b.png"), "OEBPS/a b.png");
        assert_eq!(scheme_path("epub:/OEBPS/a.png"), "OEBPS/a.png");
        let png = loaded.resource("epub://OEBPS/images/cover.png").unwrap();
        assert!(png.starts_with("data:image/png;base64,"));
        assert!(loaded.resource("epub://OEBPS/nothing.png").is_none());
    }

    #[test]
    fn remote_urls_stay_alone() {
        for url in [
            "",
            "#top",
            "data:image/png;base64,AA",
            "http://a.test/x.png",
            "https://a.test/x.png",
            "//a.test/x.png",
            "epub://OEBPS/x.png",
            "mailto:a@b.test",
        ] {
            assert!(is_remote(url), "{url}");
        }
        assert!(!is_remote("images/x.png"));
        assert_eq!(
            rewrite_assets(r#"<img src="https://a.test/x.png" alt=""/>"#, "OEBPS"),
            r#"<img src="https://a.test/x.png" alt=""/>"#
        );
    }

    #[test]
    fn resolves_paths() {
        assert_eq!(resolve("OEBPS/text", "../images/a.png"), "OEBPS/images/a.png");
        assert_eq!(percent_decode("a%20b.xhtml"), "a b.xhtml");
        assert_eq!(tag_name("</OL>"), "/ol");
    }

    #[test]
    fn decodes_declared_encoding() {
        let (gbk, _, _) = encoding_rs::GBK.encode("测试书");
        let mut bytes = b"<?xml version=\"1.0\" encoding=\"gbk\"?><title>".to_vec();
        bytes.extend_from_slice(gbk.as_ref());
        bytes.extend_from_slice(b"</title>");
        assert!(decode_xml(&bytes).contains("测试书"));
    }

}

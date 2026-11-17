//! 把一个 EPUB（ZIP）解析成 TSX 能直接渲染的数据。
//!
//! 严格 XML（container.xml / OPF / NCX）用 `roxmltree`；nav.xhtml 与章节正文
//! 是 HTML，按容错方式扫描，正文原样交给 WebView 渲染。

use std::io::{Cursor, Read, Seek};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
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

/// 每章最多内联多少图片字节，避免章节 HTML 过大。
const MAX_INLINE_IMAGE_BYTES: usize = 2 * 1024 * 1024;

/// 每章最多内联多少 CSS 字节。
const MAX_CSS_BYTES: usize = 1024 * 1024;

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
    /// 取一章的正文 HTML：只内联这一章的图片，并限制总量。
    pub fn chapter_html(&self, index: u32) -> String {
        let Some(path) = self.chapter_paths.get(index as usize) else {
            return String::new();
        };
        let Ok(mut archive) = ZipArchive::new(Cursor::new(self.bytes.as_slice())) else {
            return String::new();
        };
        let names: Vec<String> = archive
            .file_names()
            .filter_map(|name| name.ok())
            .map(|name| name.into_owned())
            .collect();
        let Ok(bytes) = read(&mut archive, &names, path) else {
            return String::new();
        };
        let raw = decode_xml(&bytes);
        let mut budget = MAX_INLINE_IMAGE_BYTES;
        let mut css_budget = MAX_CSS_BYTES;
        let css = collect_css(raw.as_ref(), path, &mut archive, &names, &mut css_budget);
        let html = prepare_html(raw.as_ref(), path, &mut archive, &names, &mut budget);
        // 纯封面/空白页给一个书名页，避免打开书后一片空白。
        if plain_text(&html).trim().is_empty() {
            return format!(
                "<h1>{}</h1><p>从左侧目录选择章节，或按 → 开始阅读。</p>",
                escape_html(&self.book.title)
            );
        }
        if css.is_empty() {
            return html;
        }
        // 书籍自带样式随正文一起交给 WebView，排在应用兜底样式之后。
        format!("<style>{css}</style>{html}")
    }
}

fn prepare_html<R: Read + Seek>(
    raw: &str,
    chapter_path: &str,
    archive: &mut ZipArchive<R>,
    names: &[String],
    budget: &mut usize,
) -> String {
    let (body, class) = extract_body(raw);
    let mut body = strip_disallowed(&body);
    if !class.is_empty() {
        // body 上的 class 在 WebView 里挂到内容容器，让书籍版式规则命中。
        body = format!("<div class=\"{}\">{}</div>", escape_html(&class), body);
    }
    inline_images(&body, &dir_of(chapter_path), archive, names, budget)
}

/// 收集一章引用的 CSS：按文档顺序拼接 `<link rel="stylesheet">` 指向的
/// 样式表和 `<style>` 块的内容。`@import` 与字体文件不展开。
fn collect_css<R: Read + Seek>(
    raw: &str,
    chapter_path: &str,
    archive: &mut ZipArchive<R>,
    names: &[String],
    budget: &mut usize,
) -> String {
    let base = dir_of(chapter_path);
    let mut out = String::new();
    let mut pos = 0;
    while let Some(start) = find_byte(raw, b'<', pos) {
        let Some(gt) = find_byte(raw, b'>', start) else { break };
        let tag = &raw[start..gt + 1];
        match tag_name(tag).as_str() {
            "link" => {
                let rel = attr_range(tag, "rel")
                    .map(|(a, b)| tag[a..b].to_ascii_lowercase())
                    .unwrap_or_default();
                if rel.split([' ', '\t']).any(|t| t == "stylesheet") {
                    if let Some((a, b)) = attr_range(tag, "href") {
                        let path = resolve(&base, &percent_decode(&tag[a..b]));
                        if let Ok(bytes) = read(archive, names, &path) {
                            if bytes.len() <= *budget {
                                *budget -= bytes.len();
                                out.push_str("/* ");
                                out.push_str(&path);
                                out.push_str(" */\n");
                                out.push_str(&String::from_utf8_lossy(&bytes));
                                out.push('\n');
                            }
                        }
                    }
                }
            }
            "style" => {
                let close = find_ci(raw, "</style", gt).unwrap_or(raw.len());
                out.push_str(&raw[gt + 1..close.min(raw.len())]);
                out.push('\n');
                pos = find_byte(raw, b'>', close).map_or(raw.len(), |i| i + 1);
                continue;
            }
            _ => {}
        }
        pos = gt + 1;
    }
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

fn inline_images<R: Read + Seek>(
    s: &str,
    base: &str,
    archive: &mut ZipArchive<R>,
    names: &[String],
    budget: &mut usize,
) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while let Some(start) = find_ci(s, "<img", pos) {
        let Some(gt) = find_byte(s, b'>', start) else { break };
        out.push_str(&s[pos..start]);
        let tag = &s[start..gt + 1];
        match rewrite_img(tag, base, archive, names, budget) {
            Some(rewritten) => out.push_str(&rewritten),
            None => out.push_str(tag),
        }
        pos = gt + 1;
    }
    out.push_str(&s[pos..]);
    out
}

fn rewrite_img<R: Read + Seek>(
    tag: &str,
    base: &str,
    archive: &mut ZipArchive<R>,
    names: &[String],
    budget: &mut usize,
) -> Option<String> {
    let (from, to) = attr_range(tag, "src")?;
    let src = percent_decode(&tag[from..to]);
    if src.is_empty()
        || src.starts_with("data:")
        || src.starts_with("http:")
        || src.starts_with("https:")
    {
        return None;
    }
    let path = resolve(base, &src);
    let bytes = read(archive, names, &path).ok()?;
    if bytes.len() > *budget {
        // 超出本章预算：丢掉这张图，别让整章 HTML 变成几十 MB。
        return Some(String::new());
    }
    *budget -= bytes.len();
    let uri = format!("data:{};base64,{}", mime_of(&path), STANDARD.encode(&bytes));
    Some(format!("{}{}{}", &tag[..from], uri, &tag[to..]))
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
    if path.ends_with(".jpg") || path.ends_with(".jpeg") {
        "image/jpeg"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".gif") {
        "image/gif"
    } else if path.ends_with(".webp") {
        "image/webp"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
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
            let mut j = i + needle.len() + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            let quote = *bytes.get(j)?;
            if quote == b'"' || quote == b'\'' {
                let from = j + 1;
                let to = tag[from..].find(quote as char)? + from;
                return Some((from, to));
            }
            let mut to = j;
            while to < bytes.len() && !bytes[to].is_ascii_whitespace() && bytes[to] != b'>' {
                to += 1;
            }
            return Some((j, to));
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
            b"p { margin: 0 0 .4em; } .lead { font-weight: bold; }",
        );
        add(
            &mut w,
            "OEBPS/ch1.xhtml",
            r#"<html><head><link rel="stylesheet" href="style.css"/><style>.note { color: #666; }</style></head><body class="calibre2"><h1>第一章</h1><p class="lead">你好，世界。</p><img src="images/cover.png" alt="封面"/><script>bad()</script></body></html>"#.as_bytes(),
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
    fn chapter_html_is_loaded_on_demand() {
        let loaded = parse(sample(false)).unwrap();
        let html = loaded.chapter_html(0);
        assert!(html.contains("你好，世界。"));
        assert!(html.contains("data:image/png;base64,"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<link"));
        assert!(!html.contains("<body"));
        // 外链样式表与内联 <style> 都被收集进 <style> 注入。
        assert!(html.contains("/* OEBPS/style.css */"));
        assert!(html.contains("font-weight: bold"));
        assert!(html.contains(".note { color: #666; }"));
        // body 上的 class 保留到内容容器，书籍版式规则才能命中。
        assert!(html.contains("<div class=\"calibre2\">"));
        assert!(loaded.chapter_html(1).contains("第二章正文"));
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

//! 把一个 EPUB（ZIP）解析成 TSX 能直接渲染的数据。
//!
//! 严格 XML（container.xml / OPF / NCX）用 `roxmltree`；nav.xhtml 与章节正文
//! 是 HTML，按容错方式扫描，正文原样交给 WebView 渲染。

use std::io::{Cursor, Read};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use nsh_core::prelude::*;
use roxmltree::Document;
use zip::ZipArchive;

type Archive = ZipArchive<Cursor<Vec<u8>>>;

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

/// 一章正文。
#[derive(Debug, Clone, JsObject)]
pub struct Chapter {
    /// 章节在 ZIP 内的路径，用来匹配目录链接。
    pub id: String,
    /// 章节标题。
    pub title: String,
    /// 清洗后的 HTML 片段，内嵌图片已变成 data URI。
    pub html: String,
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

pub fn parse(data: Vec<u8>) -> Result<Book, String> {
    let mut archive = ZipArchive::new(Cursor::new(data))
        .map_err(|e| format!("不是有效的 EPUB 文件: {e}"))?;
    let names: Vec<String> = archive
        .file_names()
        .filter_map(|name| name.ok())
        .map(|name| name.into_owned())
        .collect();

    let container = read(&mut archive, &names, "META-INF/container.xml")?;
    let container_xml = String::from_utf8_lossy(&container);
    let container = Document::parse(container_xml.as_ref())
        .map_err(|e| format!("container.xml 解析失败: {e}"))?;
    let opf_path = container
        .descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .map(percent_decode)
        .ok_or_else(|| "container.xml 缺少 rootfile".to_owned())?;
    let opf_dir = dir_of(&opf_path);

    let opf_bytes = read(&mut archive, &names, &opf_path)?;
    let opf_xml = String::from_utf8_lossy(&opf_bytes);
    let opf = Document::parse(opf_xml.as_ref())
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
        let bytes = read(&mut archive, &names, &item.href).unwrap_or_default();
        let raw = String::from_utf8_lossy(&bytes);
        let html = prepare_html(raw.as_ref(), &item.href, &mut archive, &names);
        chapters.push(Chapter {
            id: item.href.clone(),
            title: format!("第 {} 节", chapters.len() + 1),
            html,
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

    Ok(Book { title, chapters, toc })
}

fn prepare_html(raw: &str, chapter_path: &str, archive: &mut Archive, names: &[String]) -> String {
    let body = extract_body(raw);
    let body = strip_disallowed(&body);
    inline_images(&body, &dir_of(chapter_path), archive, names)
}

fn extract_body(s: &str) -> String {
    if let Some(start) = find_ci(s, "<body", 0) {
        if let Some(gt) = find_byte(s, b'>', start) {
            let end = find_ci(s, "</body", gt).unwrap_or(s.len());
            return s[gt + 1..end].to_owned();
        }
    }
    s.to_owned()
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

fn inline_images(s: &str, base: &str, archive: &mut Archive, names: &[String]) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while let Some(start) = find_ci(s, "<img", pos) {
        let Some(gt) = find_byte(s, b'>', start) else { break };
        out.push_str(&s[pos..start]);
        let tag = &s[start..gt + 1];
        match rewrite_img(tag, base, archive, names) {
            Some(rewritten) => out.push_str(&rewritten),
            None => out.push_str(tag),
        }
        pos = gt + 1;
    }
    out.push_str(&s[pos..]);
    out
}

fn rewrite_img(tag: &str, base: &str, archive: &mut Archive, names: &[String]) -> Option<String> {
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
    let Ok(doc) = Document::parse(&xml) else {
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

fn read(archive: &mut Archive, names: &[String], path: &str) -> Result<Vec<u8>, String> {
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
            r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/"><navMap><navPoint id="n1"><navLabel><text>第一章</text></navLabel><content src="ch1.xhtml"/></navPoint><navPoint id="n2"><navLabel><text>第二章</text></navLabel><content src="ch2.xhtml"/><navPoint id="n3"><navLabel><text>小节</text></navLabel><content src="ch2.xhtml#sec2"/></navPoint></navPoint></navMap></ncx>"#.as_bytes(),
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
            "OEBPS/ch1.xhtml",
            r#"<html><head><link rel="stylesheet" href="style.css"/></head><body><h1>第一章</h1><p>你好，世界。</p><img src="images/cover.png" alt="封面"/><script>bad()</script></body></html>"#.as_bytes(),
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
        let book = parse(sample(false)).unwrap();
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
        let book = parse(sample(true)).unwrap();
        assert_eq!(book.toc.len(), 3);
        assert_eq!(book.toc[2].anchor, "sec2");
        assert_eq!(book.toc[2].level, 1);
    }

    #[test]
    fn cleans_chapter_html_and_inlines_images() {
        let book = parse(sample(false)).unwrap();
        let html = &book.chapters[0].html;
        assert!(html.contains("你好，世界。"));
        assert!(html.contains("data:image/png;base64,"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("<link"));
        assert!(!html.contains("<body"));
    }

    #[test]
    fn resolves_paths() {
        assert_eq!(resolve("OEBPS/text", "../images/a.png"), "OEBPS/images/a.png");
        assert_eq!(percent_decode("a%20b.xhtml"), "a b.xhtml");
        assert_eq!(tag_name("</OL>"), "/ol");
    }
}

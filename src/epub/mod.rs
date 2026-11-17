//! 把一个 EPUB（ZIP）解析成 TSX 能直接渲染的数据。
//!
//! 严格 XML（container.xml / OPF / NCX）用 `roxmltree`；nav.xhtml 与章节
//! 正文是 HTML，按容错方式扫描。解析结果留在 Rust 侧，正文由 `render`
//! 拼成一份整书文档交给 WebView，资源按需取字节。
//!
//! 子模块：`html` 是字节级 HTML 扫描，`path` 是路径与 MIME，`render`
//! 负责整书文档的拼装与地址改写。

mod html;
mod path;
mod render;

use std::io::{Cursor, Read, Seek};

use nsh_core::prelude::*;
use roxmltree::{Document, ParsingOptions};
use zip::ZipArchive;

use self::html::{attr_range, find_byte, find_ci, plain_text, tag_name};
use self::path::{dir_of, resolve};

pub use self::path::percent_decode;

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

/// 一章的元信息；正文由 `Loaded::book_html()` 统一拼进整书文档。
#[derive(Debug, Clone, JsObject)]
pub struct Chapter {
    /// 章节在 ZIP 内的路径，用来匹配目录链接。
    pub id: String,
    /// 章节标题。
    pub title: String,
}

/// 已打开的书：原始字节留在 Rust 侧，正文与资源按需取出，
/// 避免把整本书（含图片）一次性塞给 WebView。
pub struct Loaded {
    pub book: Book,
    /// EPUB 文件的原始字节，重新打开 ZIP 用。
    bytes: Vec<u8>,
    /// 章节在 ZIP 内的路径，与 `book.chapters` 一一对应。
    chapter_paths: Vec<String>,
    /// 封面图在 ZIP 内的路径；封面页需要单独排版成一整页。
    cover: Option<String>,
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

/// 阅读文档的配色与字号，由宿主按当前主题传入；拼装时写成 CSS 变量，
/// 之后换外观或字号只需改这些变量，不必重新生成文档。
#[derive(Debug, Clone, JsObject)]
pub struct BookTheme {
    /// 纸面底色。
    pub paper: String,
    /// 正文颜色。
    pub ink: String,
    /// 次要文字（注释、页边注）。
    pub muted: String,
    /// 分隔线与引用竖线。
    pub rule: String,
    /// 链接颜色。
    pub link: String,
    /// 是否深色外观；深色下给 `<html>` 加 `dark` 类，强制正文可读。
    pub dark: bool,
    /// 正文字号，单位 pt。
    pub font_size: f64,
}

impl Default for BookTheme {
    /// 浅色纸面：没传主题时文档也能看。
    fn default() -> Self {
        Self {
            paper: "#fffdf8".to_owned(),
            ink: "#2c2a26".to_owned(),
            muted: "#8a887f".to_owned(),
            rule: "#ddd8cc".to_owned(),
            link: "#8a6a3a".to_owned(),
            dark: false,
            font_size: 18.0,
        }
    }
}

/// manifest 里的一项。
struct Item {
    id: String,
    href: String,
    media_type: String,
    properties: String,
}

/// 目录里的一项，`target` 还没对到章节上。
struct RawToc {
    label: String,
    target: String,
    level: u32,
}

pub fn parse(data: Vec<u8>) -> Result<Loaded, String> {
    let saved = data.clone();
    let mut archive =
        ZipArchive::new(Cursor::new(data)).map_err(|e| format!("不是有效的 EPUB 文件: {e}"))?;
    let names: Vec<String> = archive
        .file_names()
        .filter_map(|name| name.ok())
        .map(|name| name.into_owned())
        .collect();

    let container = read(&mut archive, &names, "META-INF/container.xml")?;
    let container_xml = decode_xml(&container);
    let container =
        parse_xml(&container_xml).map_err(|e| format!("container.xml 解析失败: {e}"))?;
    let opf_path = container
        .descendants()
        .find(|n| n.tag_name().name() == "rootfile")
        .and_then(|n| n.attribute("full-path"))
        .map(percent_decode)
        .ok_or_else(|| "container.xml 缺少 rootfile".to_owned())?;
    let opf_dir = dir_of(&opf_path);

    let opf_bytes = read(&mut archive, &names, &opf_path)?;
    let opf_xml = decode_xml(&opf_bytes);
    let opf = parse_xml(&opf_xml).map_err(|e| format!("OPF 解析失败: {e}"))?;

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

    // 封面图：EPUB3 认 manifest 的 `cover-image` 属性，EPUB2 认
    // `<meta name="cover" content="id"/>` 指到的那个条目。
    let cover = items
        .iter()
        .find(|i| i.properties.split_whitespace().any(|p| p == "cover-image"))
        .map(|i| i.href.clone())
        .or_else(|| {
            opf.descendants()
                .find(|n| n.tag_name().name() == "meta" && n.attribute("name") == Some("cover"))
                .and_then(|n| n.attribute("content"))
                .and_then(|id| items.iter().find(|i| i.id == id))
                .map(|i| i.href.clone())
        });

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

    let book = Book {
        title,
        chapters,
        toc,
    };
    let chapter_paths = book.chapters.iter().map(|c| c.id.clone()).collect();
    Ok(Loaded {
        book,
        bytes: saved,
        chapter_paths,
        cover,
    })
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
                let href = attr_range(tag, "href").map(|(a, b)| &tag[a..b]).unwrap_or("");
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
    for node in doc
        .descendants()
        .filter(|n| n.tag_name().name() == "navPoint")
    {
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

/// 目录目标 → (章节下标, 锚点)；对不上任何章节时退回第一章。
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

fn read<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    names: &[String],
    path: &str,
) -> Result<Vec<u8>, String> {
    let name = match_name(names, path);
    let mut file = archive
        .by_name(&name)
        .map_err(|e| format!("EPUB 内找不到 {path}: {e}"))?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .map_err(|e| format!("读取 {path} 失败: {e}"))?;
    Ok(buf)
}

/// ZIP 条目名大小写不一定与 OPF 一致，按路径或后缀找最接近的一个。
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

#[cfg(test)]
mod tests {
    use super::html::tag_name;
    use super::path::{resolve, scheme_path};
    use super::*;
    use std::io::Write;
    use zip::CompressionMethod;
    use zip::write::SimpleFileOptions;
    use zip::ZipWriter;

    fn add(writer: &mut ZipWriter<Cursor<Vec<u8>>>, name: &str, body: &[u8]) {
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        writer.start_file(name, options).unwrap();
        writer.write_all(body).unwrap();
    }

    /// 一本两章的小书：EPUB3 时带 nav，否则只有 NCX。
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

    /// 一本带封面页的小书：封面页只有一张 SVG 封面图，manifest 用
    /// `cover-image` 标出来。
    fn cover_sample() -> Vec<u8> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        add(
            &mut w,
            "META-INF/container.xml",
            r#"<?xml version="1.0"?><container xmlns="urn:oasis:names:tc:opendocument:xmlns:container" version="1.0"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#.as_bytes(),
        );
        add(
            &mut w,
            "OEBPS/content.opf",
            r#"<?xml version="1.0"?><package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>封面书</dc:title></metadata><manifest><item id="cover" href="cover.xhtml" media-type="application/xhtml+xml"/><item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/><item id="img" href="images/cover.png" media-type="image/png" properties="cover-image"/></manifest><spine><itemref idref="cover"/><itemref idref="ch1"/></spine></package>"#.as_bytes(),
        );
        add(
            &mut w,
            "OEBPS/cover.xhtml",
            r#"<html><body><div><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" version="1.1" width="100%" height="100%" viewBox="0 0 600 800" preserveAspectRatio="none"><image width="600" height="800" xlink:href="images/cover.png"/></svg></div></body></html>"#.as_bytes(),
        );
        add(
            &mut w,
            "OEBPS/ch1.xhtml",
            r#"<html><body><p>正文</p></body></html>"#.as_bytes(),
        );
        add(&mut w, "OEBPS/images/cover.png", &[0x89, b'P', b'N', b'G']);
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn cover_page_gets_its_own_page() {
        let loaded = parse(cover_sample()).unwrap();
        let html = loaded.book_html(&BookTheme::default());
        // 只有封面这一章带 epub-cover，正文页不带。
        assert_eq!(html.matches("class=\"epub-chapter epub-cover\"").count(), 1);
        assert!(html.contains("class=\"epub-chapter\">"));
        // SVG 封面图也走 epub:// 占位。
        assert!(html.contains("data-epub=\"epub://OEBPS/images/cover.png\""));
        // 整页缩放与分页断点写在样式里。
        assert!(html.contains("#content section.epub-cover"));
        assert!(html.contains("break-before:column"));
        // `preserveAspectRatio="none"` 会把封面拉扁，改成等比居中。
        assert!(html.contains("preserveAspectRatio=\"xMidYMid meet\""));
        assert!(!html.contains("preserveAspectRatio=\"none\""));
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
        let html = loaded.book_html(&BookTheme::default());
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
        // 主题变量写进文档，宿主换外观时只需改这几个变量。
        assert!(html.contains("--paper:#fffdf8"));
        assert!(html.contains("--font-size:18px"));
        assert!(!html.contains("class=\"dark\""));
    }

    #[test]
    fn dark_theme_marks_the_document() {
        let loaded = parse(sample(false)).unwrap();
        let dark = BookTheme {
            dark: true,
            font_size: 22.0,
            ..BookTheme::default()
        };
        let html = loaded.book_html(&dark);
        assert!(html.contains("<html class=\"dark\">"));
        assert!(html.contains("--font-size:22px"));
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
    fn resolves_paths() {
        assert_eq!(
            resolve("OEBPS/text", "../images/a.png"),
            "OEBPS/images/a.png"
        );
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

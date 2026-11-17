//! 整书渲染：把各章正文拼成一份多列文档，并改写锚点、跨章链接与资源地址。
//!
//! 拼装时做三件事：章内锚点加章节前缀、跨章链接改成本文档内的锚点、
//! 图片与音视频地址写成 `epub://` 占位（宿主取到字节后回填）。样式表里的
//! 字体与背景图等不到回填，直接内联成 `data:` URL。

use std::io::{Cursor, Read, Seek};

use zip::ZipArchive;

use super::Loaded;
use super::html::{
    attr_range, attr_whole, escape_html, extract_body, find_byte, find_ci,
    fix_double_escaped_entities, plain_text, strip_disallowed, tag_name,
};
use super::path::{data_url, dir_of, is_remote, mime_of, percent_decode, resolve, scheme_path};
use super::{BookTheme, decode_xml, read};

impl Loaded {
    /// 生成整本书的单文档 HTML：所有章节按顺序拼成一个多列文档。
    /// 图片、音视频写成 `epub://` 占位等宿主回填（`resource()`），样式表里
    /// 的字体和背景图则在拼装时直接内联成 `data:` URL。
    pub fn book_html(&self, theme: &BookTheme) -> String {
        let Ok(mut archive) = ZipArchive::new(Cursor::new(self.bytes.as_slice())) else {
            return String::new();
        };
        let names = archive_names(&mut archive);
        let css = self.book_css(&mut archive, &names);
        let mut content = String::new();
        for (i, path) in self.chapter_paths.iter().enumerate() {
            let Ok(bytes) = read(&mut archive, &names, path) else {
                continue;
            };
            let raw = decode_xml(&bytes);
            let (body, body_class) = extract_body(&raw);
            let body = fix_double_escaped_entities(&body);
            let body = strip_disallowed(&body);
            let body = prefix_ids(&body, i);
            let body = rewrite_links(&body, i, &dir_of(path), &self.chapter_paths);
            let body = rewrite_assets(&body, &dir_of(path));
            let cover_page = self.is_cover_page(&body);
            let body = if cover_page {
                keep_cover_aspect(&body)
            } else {
                body
            };
            let mut class = String::from("epub-chapter");
            if cover_page {
                class.push_str(" epub-cover");
            }
            if !body_class.is_empty() {
                class.push(' ');
                class.push_str(&escape_html(&body_class));
            }
            content.push_str(&format!(
                "<section id=\"ch{i}\" data-chapter=\"{i}\" class=\"{class}\">{body}</section>\n"
            ));
        }
        book_document(&self.book.title, &css, &content, theme)
    }

    /// 这一章是不是封面页：正文里只有那张封面图、没有文字。
    /// 封面页单独占一整页，按整页等比缩放，避免被切到好几页里。
    fn is_cover_page(&self, body: &str) -> bool {
        let Some(cover) = self.cover.as_deref() else {
            return false;
        };
        body.contains(&format!("epub://{cover}")) && plain_text(body).trim().is_empty()
    }

    /// 把文档里的 `epub://OEBPS/images/a.png` 变成可以直接塞进 `src` 的
    /// `data:` URL；ZIP 里没有这个条目时返回 `None`。
    pub fn resource(&self, url: &str) -> Option<String> {
        let path = scheme_path(url);
        if path.is_empty() {
            return None;
        }
        let mut archive = ZipArchive::new(Cursor::new(self.bytes.as_slice())).ok()?;
        let names = archive_names(&mut archive);
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

fn archive_names<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Vec<String> {
    archive
        .file_names()
        .filter_map(|name| name.ok())
        .map(|name| name.into_owned())
        .collect()
}

const BOOK_HEAD: &str =
    "><head><meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\n<title>";

const BOOK_STYLE: &str = r#"</title><style>
  html,body { margin:0; padding:0; height:100%; overflow:hidden;
    background:var(--paper,#fffdf8); color:var(--ink,#2c2a26);
    font-family:-apple-system,"PingFang SC","Songti SC",serif;
    font-size:var(--font-size,18px); line-height:1.85; }
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
  /* 封面单独占一整页：整图缩到一页之内，前后都另起一页。
     这里不能用 flex：封面的包裹层会变成收缩宽度，svg 的 100% 宽度
     跟着塌成 0，封面就整张消失了。 */
  #content section.epub-cover { display:block; height:var(--page-height,100%);
    text-align:center; break-before:column; break-after:column; break-inside:avoid; }
  /* 封面就是第一页时不要再往前断一列，否则多出一张空白页 */
  #content section.epub-cover:first-child { break-before:auto; }
  /* 封面外层的边距要抹掉：图片已经卡满一页，再带 margin 就会被切掉边缘 */
  #content section.epub-cover :where(p,div,figure) { margin:0 !important; padding:0 !important; }
  /* 只压上下限，不改 img/svg 自身的尺寸语义（改 width/height 会破坏比例） */
  #content section.epub-cover img,
  #content section.epub-cover svg { max-width:100%; max-height:var(--page-height,100%);
    break-inside:avoid; }
  :where(#content a) { color:var(--link,#8a6a3a); text-decoration:none; }
  :where(#content blockquote) { margin:1em 0; padding-left:1em; border-left:3px solid var(--rule,#ddd8cc); }
  :where(#content pre) { white-space:pre-wrap; }
  /* 深色下书籍自带的黑字会看不见：强制正文与链接跟随主题，图片不受影响 */
  html.dark #content,
  html.dark #content :where(p,div,li,span,td,th,dd,dt,blockquote,figcaption,em,strong,i,b,u,small,label,h1,h2,h3,h4,h5,h6) {
    color:var(--ink) !important; }
  html.dark #content a { color:var(--link) !important; }
</style><style>"#;

const BOOK_BODY: &str = "</style></head><body>\n<div id=\"viewport\"><div id=\"content\">";

/// 页内脚本：分页、上报页码与当前章、锚点跳转、资源回填。
///
/// 它只依赖页面自己（无框架），宿主通过 `window.__epub*` 这些入口驱动。
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
  /* 元素落在第几页：量位置时先把 translateX 归零，
     否则两侧都带着翻页位移，算出来的页码会跟着当前页漂。 */
  function pageOf(el){
    var t=content.style.transform;
    content.style.transform='translateX(0)';
    var left=el.getBoundingClientRect().left-content.getBoundingClientRect().left;
    content.style.transform=t;
    return Math.max(0,Math.floor(left/width));
  }
  /* 找不到锚点时退到它所属的章（`ch3-x` → 第 3 章开头）。Calibre 这类
     工具生成的目录，锚点常挂在被剥掉的 <body id> 上，这类目标本来
     就是章首；直接退回全书第一页会让整本目录都点不动。 */
  function goAnchor(id){
    var el=id?document.getElementById(id):null;
    if(!el){
      var m=/^ch(\d+)/.exec(id||'');
      el=m?sections[+m[1]]:null;
      if(!el){ setPage(0,true); return; }
    }
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
fn book_document(title: &str, css: &str, content: &str, theme: &BookTheme) -> String {
    let mut out = String::with_capacity(content.len() + css.len() + 8192);
    out.push_str("<!doctype html><html");
    if theme.dark {
        out.push_str(" class=\"dark\"");
    }
    out.push_str(BOOK_HEAD);
    out.push_str(&escape_html(title));
    out.push_str(BOOK_STYLE);
    out.push_str(&theme_vars(theme));
    out.push_str(css);
    out.push_str(BOOK_BODY);
    out.push_str(content);
    out.push_str(BOOK_SCRIPT);
    out.push_str("</body></html>");
    out
}

/// 主题变量：宿主换外观或字号时改的就是这几个名字，两边的约定要保持一致。
fn theme_vars(theme: &BookTheme) -> String {
    format!(
        ":root {{ --paper:{}; --ink:{}; --muted:{}; --rule:{}; --link:{}; --font-size:{}px }}\n",
        theme.paper, theme.ink, theme.muted, theme.rule, theme.link, theme.font_size
    )
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

/// Calibre、InDesign 生成的封面常写成
/// `<svg width="100%" height="100%" preserveAspectRatio="none" viewBox="…">`，
/// `none` 会把图按容器比例拉扁。封面要等比，这里改成 `xMidYMid meet`，
/// 让 svg 自己在页内留白居中。
fn keep_cover_aspect(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    let mut pos = 0;
    while let Some(start) = find_byte(body, b'<', pos) {
        let Some(gt) = find_byte(body, b'>', start) else { break };
        out.push_str(&body[pos..start]);
        let mut tag = body[start..gt + 1].to_owned();
        if tag_name(&tag) == "svg"
            && let Some((a, b)) = attr_range(&tag, "preserveAspectRatio")
            && tag[a..b].trim().eq_ignore_ascii_case("none")
        {
            tag.replace_range(a..b, "xMidYMid meet");
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn assets_become_scheme_placeholders() {
        let html = rewrite_assets(r#"<img src="../images/a.png" alt=""/>"#, "OEBPS/text");
        assert!(html.contains(r#"data-epub="epub://OEBPS/images/a.png""#));
        assert!(html.contains(r#"data-epub-attr="src""#));
        assert!(!html.contains("src="));
    }
}

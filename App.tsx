import { keymap, List, mount, signal, Text, View, WebView } from "nsh";
import { Button } from "nsh/base";
import { Epub } from "epubviewer/native";
import type { Book, TocEntry } from "epubviewer/native";

type WebHandle = {
  injectJavaScript(script: string): void;
  evaluate(script: string): Promise<{ value: unknown }>;
};
type Message = { type: string; total?: number; page?: number; href?: string };
type Pending = { chapter: number; anchor: string } | null;

let web: WebHandle | null = null;
let reportedFor = -1;
const book = signal<Book | null>(null);
const chapterIndex = signal(0);
const page = signal(0);
const totalPages = signal(1);
const showToc = signal(true);
const busy = signal(false);
const error = signal<string | null>(null);
const reload = signal(0);
const pending = signal<Pending>(null);

// 把章节 HTML 包成「一页一张纸」：CSS 多列把整章排成若干列，
/// 每列正好一页，translateX 切到第 n 列。页数由页内脚本算好后上报。
function pageHtml(body: string, stamp: number): string {
  return `<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
  html,body { margin:0; padding:0; height:100%; overflow:hidden;
    background:#fffdf8; color:#2c2a26;
    font-family:-apple-system,"PingFang SC","Songti SC",serif;
    font-size:18px; line-height:1.85; }
  #viewport { position:fixed; inset:44px 56px; overflow:hidden; }
  #content { height:100%; column-gap:0; column-fill:auto; transform:translateX(0); }
  #content p { margin:0 0 1em; text-align:justify; }
  #content h1,#content h2,#content h3 { line-height:1.4; margin:1.2em 0 .6em; }
  #content img { max-width:100%; height:auto; }
  #content a { color:#8a6a3a; text-decoration:none; }
  #content blockquote { margin:1em 0; padding-left:1em; border-left:3px solid #ddd8cc; }
  #content pre { white-space:pre-wrap; }
</style></head><body>
<div id="viewport"><div id="content">${body}</div></div>
<!-- ${stamp} -->
<script>
(function(){
  var viewport=document.getElementById('viewport');
  var content=document.getElementById('content');
  var width=1,total=1,current=0;
  function post(msg){ if(window.nsh&&window.nsh.postMessage) window.nsh.postMessage(JSON.stringify(msg)); }
  function setPage(n,report){
    current=Math.max(0,Math.min(n,total-1));
    content.style.transform='translateX('+(-current*width)+'px)';
    if(report) post({type:'page',page:current,total:total});
  }
  function layout(){
    width=Math.max(1,viewport.clientWidth);
    content.style.width=width+'px';
    content.style.columnWidth=width+'px';
    content.style.height=viewport.clientHeight+'px';
    total=Math.max(1,Math.ceil((content.scrollWidth-1)/width));
    if(current>total-1) current=total-1;
    setPage(current,false);
    post({type:'ready',total:total,page:current});
  }
  window.__epubSetPage=function(n){ setPage(n,true); };
  window.__epubReport=function(){ return {total:total,page:current}; };
  window.__epubLayout=layout;
  window.__epubGoToAnchor=function(id){
    if(!id){ setPage(0,true); return; }
    var el=document.getElementById(id)||document.querySelector('[name="'+id+'"]');
    var rect=el&&el.getClientRects()[0];
    if(!rect){ setPage(0,true); return; }
    var left=rect.left-viewport.getBoundingClientRect().left+current*width;
    setPage(Math.floor(left/width),true);
  };
  document.addEventListener('click',function(e){
    var a=e.target&&e.target.closest?e.target.closest('a'):null;
    if(!a) return;
    e.preventDefault();
    post({type:'link',href:a.getAttribute('href')||''});
  });
  window.addEventListener('resize',layout);
  window.addEventListener('load',function(){ setTimeout(layout,0); });
  var imgs=content.querySelectorAll('img');
  for(var i=0;i<imgs.length;i++) imgs[i].addEventListener('load',layout);
  setTimeout(layout,0);
})();
</script></body></html>`;
}

function welcomeHtml(): string {
  return pageHtml(
    '<h1>ePubViewer</h1><p>点左上角「打开 EPUB」，或把 .epub 文件拖到窗口里。</p>' +
      '<p>← / →、空格翻页，左侧目录可跳转到任意章节。</p>',
    0,
  );
}

function chapter() {
  return book()?.chapters[chapterIndex()];
}

function adopt(next: Book) {
  book.set(next);
  chapterIndex.set(0);
  page.set(0);
  totalPages.set(1);
  pending.set({ chapter: 0, anchor: "" });
  reload.update((n) => n + 1);
}

async function openBook() {
  busy.set(true);
  error.set(null);
  try {
    const picked = await Epub.open();
    if (picked) adopt(picked);
  } catch (e) {
    error.set(String(e));
  } finally {
    busy.set(false);
  }
}

async function openDropped(uri: string) {
  busy.set(true);
  error.set(null);
  try {
    adopt(await Epub.load(uri));
  } catch (e) {
    error.set(String(e));
  } finally {
    busy.set(false);
  }
}

function go(n: number) {
  if (!book()) return;
  const clamped = Math.max(0, Math.min(n, totalPages() - 1));
  page.set(clamped);
  web?.injectJavaScript(`window.__epubSetPage && window.__epubSetPage(${clamped})`);
}

function jump(entry: TocEntry) {
  chapterIndex.set(entry.chapter);
  page.set(0);
  pending.set({ chapter: entry.chapter, anchor: entry.anchor });
  reload.update((n) => n + 1);
}

function jumpHref(href: string) {
  if (!href || href.startsWith("http")) return;
  if (href.startsWith("#")) {
    web?.injectJavaScript(
      `window.__epubGoToAnchor && window.__epubGoToAnchor(${JSON.stringify(href.slice(1))})`,
    );
    return;
  }
  const [rawFile, anchor = ""] = href.split("#");
  const file = decodeURIComponent(rawFile);
  const tail = file.split("/").pop() ?? file;
  const current = book();
  if (!current) return;
  const index = current.chapters.findIndex(
    (c) => c.id === file || c.id === tail || c.id.endsWith("/" + tail),
  );
  if (index < 0) return;
  chapterIndex.set(index);
  page.set(0);
  pending.set({ chapter: index, anchor });
  reload.update((n) => n + 1);
}

function onMessage(data: string) {
  let msg: Message;
  try {
    msg = JSON.parse(data);
  } catch {
    return;
  }
  if (msg.type === "ready") {
    totalPages.set(Math.max(1, msg.total ?? 1));
    const stamp = reload();
    if (reportedFor === stamp) return;
    reportedFor = stamp;
    const target = pending();
    pending.set(null);
    if (target && target.anchor) {
      web?.injectJavaScript(
        `window.__epubGoToAnchor && window.__epubGoToAnchor(${JSON.stringify(target.anchor)})`,
      );
    } else {
      go(page());
    }
  } else if (msg.type === "page") {
    totalPages.set(Math.max(1, msg.total ?? totalPages()));
    page.set(msg.page ?? 0);
  } else if (msg.type === "link") {
    jumpHref(msg.href ?? "");
  }
}

function App() {
  keymap.bind(
    {
      left: () => go(page() - 1),
      right: () => go(page() + 1),
      space: () => go(page() + 1),
      pageup: () => go(page() - 1),
      pagedown: () => go(page() + 1),
      "mod+o": () => void openBook(),
      "mod+t": () => showToc.update((v) => !v),
    },
    { when: "always" },
  );

  return (
    <View
      style={{ flex: 1, flexDirection: "row", backgroundColor: "#eceae4" }}
      onFileDrop={(e) => {
        const uri = e.uris[0];
        if (uri) void openDropped(uri);
      }}
    >
      {showToc() ? (
        <View
          style={{
            width: 268,
            backgroundColor: "#f7f6f2",
            borderRightWidth: 1,
            borderColor: "#dedbd2",
          }}
        >
          <View style={{ padding: 18, paddingBottom: 10 }}>
            <Text style={{ fontSize: 13, color: "#8a887f" }}>目录</Text>
          </View>
          <List each={book()?.toc ?? []} style={{ flex: 1 }} keyExtractor={(item) => item.label}>
            {(item) => (
              <Button
                onPress={() => jump(item())}
                style={{
                  paddingLeft: 16 + item().level * 14,
                  paddingRight: 16,
                  paddingTop: 7,
                  paddingBottom: 7,
                }}
              >
                <Text style={{ fontSize: 13, color: "#3b3a35" }}>{item().label}</Text>
              </Button>
            )}
          </List>
        </View>
      ) : null}

      <View style={{ flex: 1, flexDirection: "column" }}>
        <View
          style={{
            height: 52,
            flexDirection: "row",
            alignItems: "center",
            gap: 12,
            paddingLeft: 18,
            paddingRight: 18,
            borderBottomWidth: 1,
            borderColor: "#dedbd2",
            backgroundColor: "#f7f6f2",
          }}
        >
          <Button
            onPress={() => void openBook()}
            style={{
              paddingLeft: 14,
              paddingRight: 14,
              paddingTop: 8,
              paddingBottom: 8,
              borderRadius: 8,
              backgroundColor: "#2f2e2b",
            }}
          >
            <Text style={{ fontSize: 13, color: "#ffffff" }}>
              {busy() ? "打开中…" : "打开 EPUB"}
            </Text>
          </Button>
          <Button
            onPress={() => showToc.update((v) => !v)}
            style={{ paddingLeft: 10, paddingRight: 10, paddingTop: 8, paddingBottom: 8 }}
          >
            <Text style={{ fontSize: 13, color: "#5a5851" }}>目录</Text>
          </Button>
          <Text style={{ flex: 1, fontSize: 14, color: "#2f2e2b" }}>
            {book() ? book()!.title : "尚未打开文件"}
          </Text>
          {error() ? <Text style={{ fontSize: 12, color: "#b23b3b" }}>{error()}</Text> : null}
          <Text style={{ fontSize: 12, color: "#8a887f" }}>{chapter()?.title ?? ""}</Text>
        </View>

        <View style={{ flex: 1, padding: 22 }}>
          <View style={{ flex: 1, backgroundColor: "#fffdf8", borderRadius: 6 }}>
            <WebView
              ref={(node) => {
                web = node;
              }}
              html={chapter() ? pageHtml(chapter()!.html, reload()) : welcomeHtml()}
              scrollable={false}
              style={{ flex: 1 }}
              onLoad={() => {
                web?.injectJavaScript("window.__epubLayout && window.__epubLayout();");
                web
                  ?.evaluate("window.__epubReport ? window.__epubReport() : null")
                  .then((result) => {
                    const value = result.value as { total?: number; page?: number } | null;
                    if (value) onMessage(JSON.stringify({ type: "ready", ...value }));
                  })
                  .catch(() => {});
              }}
              onMessage={(e) => onMessage(e.data)}
              onError={(e) => error.set(e.message)}
            />
            <View
              onPress={() => go(page() - 1)}
              style={{ position: "absolute", left: 0, top: 0, width: 56, height: "100%" }}
            />
            <View
              onPress={() => go(page() + 1)}
              style={{ position: "absolute", right: 0, top: 0, width: 56, height: "100%" }}
            />
          </View>
        </View>

        <View
          style={{
            height: 48,
            flexDirection: "row",
            alignItems: "center",
            justifyContent: "center",
            gap: 18,
            borderTopWidth: 1,
            borderColor: "#dedbd2",
            backgroundColor: "#f7f6f2",
          }}
        >
          <Button onPress={() => go(page() - 1)} style={{ padding: 8 }}>
            <Text style={{ fontSize: 13, color: "#5a5851" }}>‹ 上一页</Text>
          </Button>
          <Text style={{ fontSize: 13, color: "#3b3a35" }}>
            {page() + 1} / {totalPages()}
          </Text>
          <Button onPress={() => go(page() + 1)} style={{ padding: 8 }}>
            <Text style={{ fontSize: 13, color: "#5a5851" }}>下一页 ›</Text>
          </Button>
          <Text style={{ fontSize: 12, color: "#8a887f" }}>
            {book() ? `${chapterIndex() + 1} / ${book()!.chapters.length} 章` : ""}
          </Text>
        </View>
      </View>
    </View>
  );
}

mount(App);

import { keymap, List, mount, signal, Text, View, WebView } from "nsh";
import { Button } from "nsh/base";
import { Epub } from "epubviewer/native";
import type { Book, TocEntry } from "epubviewer/native";

type WebHandle = {
  injectJavaScript(script: string): void;
};
type Asset = { id: string; url: string };
type Message = {
  type: string;
  total?: number;
  page?: number;
  chapter?: number;
  href?: string;
  items?: Asset[];
};

let web: WebHandle | null = null;
// 已经处理过 ready 的文档戳，避免重复跳转。
let reportedFor = -1;
let docStamp = 0;

const WELCOME = `<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
  html,body { margin:0; height:100%; background:#fffdf8; color:#2c2a26;
    font-family:-apple-system,"PingFang SC","Songti SC",serif; font-size:16px; line-height:1.8; }
  main { padding:56px; }
  h1 { font-size:20px; margin:0 0 12px; }
  p { margin:0 0 8px; color:#5a5851; }
</style></head><body><main>
<h1>ePubViewer</h1>
<p>点左上角「打开 EPUB」，或把 .epub 文件拖到窗口里。</p>
<p>← / →、空格翻页，右上角目录可跳到任意章节。</p>
</main></body></html>`;

// 整本书是一份文档：分页、页码、当前章都由页内脚本算好后上报。
const book = signal<Book | null>(null);
const doc = signal<{ html: string; stamp: number }>({ html: "", stamp: 0 });
const chapterIndex = signal(0);
const page = signal(0);
const totalPages = signal(1);
const showToc = signal(false);
const busy = signal(false);
const error = signal<string | null>(null);
// 文档还没排版好就先记下要跳的锚点，ready 之后再执行。
const pendingAnchor = signal<string | null>(null);

function ready(): boolean {
  return doc().stamp !== 0 && reportedFor === doc().stamp;
}

function chapter() {
  return book()?.chapters[chapterIndex()];
}

async function loadBook(next: Book) {
  book.set(next);
  chapterIndex.set(0);
  page.set(0);
  totalPages.set(1);
  reportedFor = -1;
  pendingAnchor.set(null);
  try {
    const html = await Epub.book();
    doc.set({ html, stamp: ++docStamp });
  } catch (e) {
    error.set(String(e));
  }
}

/// 宿主把 `epub://` 换成 data: URL，再让页内脚本填回原属性。
async function hydrate(items: Asset[], stamp: number) {
  for (const item of items) {
    if (doc().stamp !== stamp) return;
    let data: string | null = null;
    try {
      data = await Epub.resource(item.url);
    } catch {
      continue;
    }
    if (!data || doc().stamp !== stamp) continue;
    web?.injectJavaScript(
      `window.__epubRes && window.__epubRes(${JSON.stringify(item.id)}, ${JSON.stringify(data)})`,
    );
  }
}

async function openBook() {
  busy.set(true);
  error.set(null);
  try {
    const picked = await Epub.open();
    if (picked) await loadBook(picked);
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
    await loadBook(await Epub.load(uri));
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

function turn(delta: number) {
  if (!book() || !ready()) return;
  go(page() + delta);
}

/// 目录项 → 文档内锚点：章节开头是 `ch{i}`，章内锚点是 `ch{i}-{anchor}`。
function jump(entry: TocEntry) {
  showToc.set(false);
  if (!book()) return;
  chapterIndex.set(entry.chapter);
  const anchor = entry.anchor ? `ch${entry.chapter}-${entry.anchor}` : `ch${entry.chapter}`;
  if (!ready()) {
    pendingAnchor.set(anchor);
    return;
  }
  web?.injectJavaScript(
    `window.__epubGoToAnchor && window.__epubGoToAnchor(${JSON.stringify(anchor)})`,
  );
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
    page.set(msg.page ?? 0);
    chapterIndex.set(msg.chapter ?? 0);
    const stamp = doc().stamp;
    if (reportedFor === stamp) return;
    reportedFor = stamp;
    const anchor = pendingAnchor();
    pendingAnchor.set(null);
    if (anchor) {
      web?.injectJavaScript(
        `window.__epubGoToAnchor && window.__epubGoToAnchor(${JSON.stringify(anchor)})`,
      );
    }
  } else if (msg.type === "page") {
    totalPages.set(Math.max(1, msg.total ?? totalPages()));
    page.set(msg.page ?? 0);
    chapterIndex.set(msg.chapter ?? chapterIndex());
  } else if (msg.type === "res") {
    void hydrate(msg.items ?? [], doc().stamp);
  }
}

function App() {
  keymap.bind(
    {
      left: () => turn(-1),
      right: () => turn(1),
      space: () => turn(1),
      pageup: () => turn(-1),
      pagedown: () => turn(1),
      "mod+o": () => void openBook(),
      "mod+t": () => {
        if (book()) showToc.update((v) => !v);
      },
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
          <Text style={{ flex: 1, fontSize: 14, color: "#2f2e2b" }}>
            {book() ? book()!.title : "尚未打开文件"}
          </Text>
          {error() ? <Text style={{ fontSize: 12, color: "#b23b3b" }}>{error()}</Text> : null}
          <Text style={{ fontSize: 12, color: "#8a887f" }}>{chapter()?.title ?? ""}</Text>
          <Button
            onPress={() => {
              if (book()) showToc.update((v) => !v);
            }}
            style={{
              paddingLeft: 12,
              paddingRight: 12,
              paddingTop: 8,
              paddingBottom: 8,
              borderRadius: 8,
              backgroundColor: showToc() ? "#e6e2d4" : undefined,
            }}
          >
            <Text style={{ fontSize: 13, color: "#5a5851" }}>目录</Text>
          </Button>
        </View>

        <View style={{ flex: 1, padding: 22 }}>
          <View
            style={{ flex: 1, backgroundColor: "#fffdf8", borderRadius: 6 }}
          >
            <WebView
              ref={(node) => {
                web = node;
              }}
              html={doc().html || WELCOME}
              scrollable={false}
              style={{ flex: 1 }}
              onLoad={() => {
                // 页面脚本的首次投递可能早于宿主接上，这里再要一次资源。
                web?.injectJavaScript(
                  "(window.__epubRequestAssets && window.__epubRequestAssets(true),window.__epubLayout && window.__epubLayout());",
                );
              }}
              onMessage={(e) => onMessage(e.data)}
              onError={(e) => error.set(e.message)}
            />
            <View
              onPress={() => turn(-1)}
              style={{ position: "absolute", left: 0, top: 0, width: 56, height: "100%" }}
            />
            <View
              onPress={() => turn(1)}
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
          <Button onPress={() => turn(-1)} style={{ padding: 8 }}>
            <Text style={{ fontSize: 13, color: "#5a5851" }}>‹ 上一页</Text>
          </Button>
          <Text style={{ fontSize: 13, color: "#3b3a35" }}>
            {`${page() + 1} / ${totalPages()}`}
          </Text>
          <Button onPress={() => turn(1)} style={{ padding: 8 }}>
            <Text style={{ fontSize: 13, color: "#5a5851" }}>下一页 ›</Text>
          </Button>
        </View>
      </View>

      {showToc() ? (
        <View
          style={{ position: "absolute", left: 0, right: 0, top: 0, bottom: 0 }}
          onPress={() => showToc.set(false)}
        />
      ) : null}
      {showToc() ? (
        <View
          style={{
            position: "absolute",
            top: 58,
            right: 12,
            width: 320,
            height: 600,
            flexDirection: "column",
            backgroundColor: "#f7f6f2",
            borderRadius: 10,
            borderWidth: 1,
            borderColor: "#dedbd2",
            overflow: "hidden",
          }}
        >
          <View
            style={{
              paddingLeft: 16,
              paddingRight: 16,
              paddingTop: 12,
              paddingBottom: 6,
              flexDirection: "row",
            }}
          >
            <Text style={{ flex: 1, fontSize: 12, color: "#8a887f" }}>目录</Text>
            <Text style={{ fontSize: 12, color: "#a5a29a" }}>{`${book()?.toc.length ?? 0} 项`}</Text>
          </View>
          <List
            each={book()?.toc ?? []}
            style={{ flex: 1, paddingBottom: 8 }}
            empty={
              <View style={{ padding: 16 }}>
                <Text style={{ fontSize: 13, color: "#8a887f" }}>这本书没有目录条目</Text>
              </View>
            }
          >
            {(item) => {
              const current = item().chapter === chapterIndex();
              return (
                <Button
                  onPress={() => jump(item())}
                  style={{
                    paddingLeft: 16 + item().level * 14,
                    paddingRight: 16,
                    paddingTop: 7,
                    paddingBottom: 7,
                    backgroundColor: current ? "#e6e2d4" : undefined,
                  }}
                >
                  <Text
                    style={{
                      fontSize: 13,
                      fontWeight: current ? "bold" : "normal",
                      color: current ? "#1f1e1b" : "#3b3a35",
                    }}
                  >
                    {item().label}
                  </Text>
                </Button>
              );
            }}
          </List>
        </View>
      ) : null}
    </View>
  );
}

mount(App);

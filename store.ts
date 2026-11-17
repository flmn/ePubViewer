//! 阅读器的领域状态与命令。
//!
//! 页面与组件只读这里导出的 signal、调用这里导出的函数。整本书的 HTML 由
//! Rust 侧生成；真正持有 WebView 的是 `components/BookView.tsx`，它通过
//! `attachViewer()` 把驱动文档的能力交回来，命令才能让页面翻页。

import { colorScheme, computed, setColorScheme, signal, Storage, token } from "nsh";
import { Epub } from "epubviewer/native";
import type { Book, Chapter, TocEntry } from "epubviewer/native";

/** 页面脚本为某个资源发来的请求。 */
type Asset = { id: string; url: string };

/** 驱动阅读文档的能力，由持有 WebView 的组件提供。 */
type Viewer = { injectJavaScript(script: string): void };

/** 打开的书；`null` 表示还没有打开文件。 */
export const book = signal<Book | null>(null);
/** 当前页落在第几章，由页内脚本上报。 */
export const chapterIndex = signal(0);
/** 当前页，从 0 起按全书连续编号。 */
export const page = signal(0);
/** 全书页数，由页内脚本上报。 */
export const totalPages = signal(1);
/** 目录浮层是否展开。 */
export const tocOpen = signal(false);
/** 外观与字号设置面板是否展开。 */
export const settingsOpen = signal(false);
/** 正在打开文件。 */
export const busy = signal(false);
/** 最近一次失败，直接显示给用户。 */
export const error = signal<string | null>(null);
/** 整本书的文档与它的戳；戳变了表示文档要重新排版。 */
export const doc = signal<{ html: string; stamp: number }>({ html: "", stamp: 0 });

/** 颜色模式：跟随系统，或固定浅色/深色。 */
export type ColorMode = "system" | "light" | "dark";

/** 用户选的颜色模式；实际外观以 `darkMode()` 为准。 */
export const colorMode = signal<ColorMode>("system");
/** 默认正文字号，单位 pt。 */
export const DEFAULT_FONT_SIZE = 18;
/** 正文字号，单位 pt。 */
export const fontSize = signal(DEFAULT_FONT_SIZE);
/** 字号是否就是默认值，用来把「重置」按钮画淡。 */
export const fontDefault = computed(() => fontSize() === DEFAULT_FONT_SIZE);
/** 当前是否深色外观：固定模式优先，`system` 跟随系统。 */
export const darkMode = computed(() => colorScheme() === "dark");

const EMPTY_TOC: TocEntry[] = [];

/** 全书目录；没有书时是一个稳定的空数组。 */
export const toc = computed<TocEntry[]>(() => book()?.toc ?? EMPTY_TOC);
/** 当前章；没有书时为 `null`。 */
export const chapter = computed<Chapter | null>(() => book()?.chapters[chapterIndex()] ?? null);

/** 已排版完成并处理过 `ready` 的文档戳。 */
let reportedStamp = -1;
/** 文档戳，每打开一本书递增。 */
let stamp = 0;
/** 驱动文档的 WebView；`null` 表示文档视图还没接上或已经没了。 */
let viewer: Viewer | null = null;
/** 文档还没排版好时先记下的锚点，`ready` 后再跳。 */
let pendingAnchor: string | null = null;

/** 由 `BookView` 在 ref 变化时调用。 */
export function attachViewer(next: Viewer | null) {
  viewer = next;
}

/** 向文档页注入一段脚本；视图还没接上时什么也不做。 */
function inject(script: string) {
  viewer?.injectJavaScript(script);
}

/** 当前文档是否已经排版完成，页码与当前章才可信。 */
function laidOut(): boolean {
  return stamp !== 0 && reportedStamp === stamp;
}

/** 弹出系统文件对话框，读取并解析一本 EPUB；取消时什么也不做。 */
export async function openBook() {
  busy.set(true);
  error.set(null);
  try {
    const picked = await Epub.open();
    if (picked) await adopt(picked);
  } catch (e) {
    error.set(String(e));
  } finally {
    busy.set(false);
  }
}

/** 打开拖进窗口的本地路径（也接受 `file://` URL）。 */
export async function openPath(uri: string) {
  busy.set(true);
  error.set(null);
  try {
    await adopt(await Epub.load(uri));
  } catch (e) {
    error.set(String(e));
  } finally {
    busy.set(false);
  }
}

/** 展开或收起目录浮层。 */
export function toggleToc() {
  if (book()) tocOpen.update((open) => !open);
}

/** 展开或收起外观与字号设置。 */
export function toggleSettings() {
  settingsOpen.update((open) => !open);
}

/** 切颜色模式，并记住选择。 */
export function setColorMode(mode: ColorMode) {
  colorMode.set(mode);
  setColorScheme(mode);
  void Storage.setItem(COLOR_MODE_KEY, mode).catch(() => {});
}

/** 调字号（`delta` 为步长），并记住结果。 */
export function adjustFont(delta: number) {
  setFont(fontSize() + delta);
}

/** 字号回到默认值，并记住结果。 */
export function resetFont() {
  setFont(DEFAULT_FONT_SIZE);
}

/** 读回上次的颜色模式与字号；没有记录时用默认值。 */
export async function loadSettings() {
  try {
    const [mode, size] = await Promise.all([
      Storage.getItem(COLOR_MODE_KEY),
      Storage.getItem(FONT_SIZE_KEY),
    ]);
    if (mode === "light" || mode === "dark" || mode === "system") setColorMode(mode);
    const parsed = size === null ? Number.NaN : Number.parseInt(size, 10);
    if (Number.isFinite(parsed)) setFont(parsed);
  } catch {
    // 读不到就用默认值，不影响阅读。
  }
}

/** 跳到第 `n` 页，会夹在全书的范围里。 */
export function go(n: number) {
  if (!book()) return;
  const target = Math.max(0, Math.min(n, totalPages() - 1));
  page.set(target);
  inject(`window.__epubSetPage && window.__epubSetPage(${target})`);
}

/** 翻 `delta` 页；到全书首尾为止。 */
export function turn(delta: number) {
  go(page() + delta);
}

/** 跳到目录项：章节开头是 `ch{i}`，章内锚点是 `ch{i}-{anchor}`。 */
export function jump(entry: TocEntry) {
  tocOpen.set(false);
  if (!book()) return;
  chapterIndex.set(entry.chapter);
  const anchor = entry.anchor ? `ch${entry.chapter}-${entry.anchor}` : `ch${entry.chapter}`;
  if (!laidOut()) {
    pendingAnchor = anchor;
    return;
  }
  goToAnchor(anchor);
}

/** 文档加载完：补发一次资源请求，并把外观与字号推给页面。 */
export function loadDocument() {
  inject("window.__epubRequestAssets && window.__epubRequestAssets(true);");
  applyDocumentTheme();
}

/** 把外观与字号推给已经打开的文档，并重新分页。 */
export function applyDocumentTheme() {
  const vars = JSON.stringify(docVars());
  inject(
    `(function(){var r=document.documentElement,v=${vars};` +
      `r.classList.toggle('dark',${darkMode()});` +
      `for(var k in v)r.style.setProperty(k,v[k]);` +
      `window.__epubLayout&&window.__epubLayout();})()`,
  );
}

/** 页内脚本上报的消息。 */
export function onEvent(data: string) {
  let message: Message;
  try {
    message = JSON.parse(data) as Message;
  } catch {
    return;
  }
  switch (message.type) {
    case "ready": {
      totalPages.set(Math.max(1, message.total ?? 1));
      page.set(message.page ?? 0);
      chapterIndex.set(message.chapter ?? 0);
      const current = doc().stamp;
      if (reportedStamp === current) return;
      reportedStamp = current;
      const anchor = pendingAnchor;
      pendingAnchor = null;
      if (anchor) goToAnchor(anchor);
      return;
    }
    case "page":
      totalPages.set(Math.max(1, message.total ?? totalPages()));
      page.set(message.page ?? 0);
      chapterIndex.set(message.chapter ?? chapterIndex());
      return;
    case "res":
      void hydrate(message.items ?? []);
      return;
    default:
      // `link`：站外链接目前没有声明 openUrl 能力，先不接管。
      return;
  }
}

/** 页内脚本上报的消息形状。 */
type Message = {
  type: string;
  total?: number;
  page?: number;
  chapter?: number;
  items?: Asset[];
};

/** 读一本书并通知视图换文档；进度归零。 */
async function adopt(next: Book) {
  book.set(next);
  chapterIndex.set(0);
  page.set(0);
  totalPages.set(1);
  reportedStamp = -1;
  pendingAnchor = null;
  try {
    const html = await Epub.book(bookTheme());
    doc.set({ html, stamp: ++stamp });
  } catch (e) {
    error.set(String(e));
  }
}

/** 字号范围。 */
const FONT_MIN = 13;
const FONT_MAX = 26;
/** 设置存在本地存储里的键。 */
const COLOR_MODE_KEY = "reader.colorMode";
const FONT_SIZE_KEY = "reader.fontSize";

function clampFont(size: number): number {
  return Math.max(FONT_MIN, Math.min(FONT_MAX, Math.round(size)));
}

/** 设正文字号：夹在范围内，改了就记住。 */
function setFont(size: number) {
  const next = clampFont(size);
  if (next === fontSize()) return;
  fontSize.set(next);
  void Storage.setItem(FONT_SIZE_KEY, String(next)).catch(() => {});
}

/** 阅读文档的 CSS 变量；名字与 Rust 侧 `theme_vars()` 一一对应。 */
function docVars() {
  return {
    "--paper": token("card"),
    "--ink": token("foreground"),
    "--muted": token("muted-foreground"),
    "--rule": token("border"),
    "--link": token("link"),
    "--font-size": `${fontSize()}px`,
  };
}

/** 生成文档时用的主题：首屏就是正确的纸色与字号，不必等注入。 */
function bookTheme() {
  return {
    paper: token("card"),
    ink: token("foreground"),
    muted: token("muted-foreground"),
    rule: token("border"),
    link: token("link"),
    dark: darkMode(),
    fontSize: fontSize(),
  };
}

/** 把 `epub://` 占位逐个换成 `data:` URL 填回页面。 */
async function hydrate(items: Asset[]) {
  const target = doc().stamp;
  for (const item of items) {
    if (doc().stamp !== target) return;
    let data: string | null = null;
    try {
      data = await Epub.resource(item.url);
    } catch {
      // 单个资源取不到不影响其余内容。
      continue;
    }
    if (!data || doc().stamp !== target) continue;
    inject(
      `window.__epubRes && window.__epubRes(${JSON.stringify(item.id)}, ${JSON.stringify(data)})`,
    );
  }
}

/** 让页内脚本跳到锚点。 */
function goToAnchor(anchor: string) {
  inject(
    `window.__epubGoToAnchor && window.__epubGoToAnchor(${JSON.stringify(anchor)})`,
  );
}

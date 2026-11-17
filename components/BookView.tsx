//! 阅读区的 WebView：载入整书文档，把页面消息与视图本身交回调用方。
//!
//! 它不读 store——文档、消息与错误都由 `pages/Reader.tsx` 接上，页面才是
//! 阅读进度的 owner。

import { token, WebView } from "nsh";

/** 驱动文档页内脚本的能力（与 `WebViewRef` 结构兼容）。 */
export type Viewer = { injectJavaScript(script: string): void };

export type BookViewProps = {
  /** 要显示的文档 HTML；空串时显示欢迎页。 */
  html: string;
  /** 拿到或失去 WebView；`null` 表示视图已经不在了。 */
  onView(view: Viewer | null): void;
  /** 页面载入完成，可以下发布局与资源请求。 */
  onLoad(): void;
  /** 页面脚本上报的 JSON 消息。 */
  onEvent(data: string): void;
  /** 页面载入失败。 */
  onError(message: string): void;
};

/** 还没打开书时的页面：说明能做什么，配色跟随主题。 */
function welcomeHtml(): string {
  return `<!doctype html><html><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
  html,body { margin:0; height:100%; background:${token("card")}; color:${token("foreground")};
    font-family:-apple-system,"PingFang SC","Songti SC",serif; font-size:16px; line-height:1.8; }
  /* 窗口很宽时（对开）也别把说明文字拉成一整行 */
  main { padding:56px; max-width:520px; }
  h1 { font-size:20px; margin:0 0 12px; }
  p { margin:0 0 8px; color:${token("muted-foreground")}; }
</style></head><body><main>
<h1>ePubViewer</h1>
<p>点左上角「打开 EPUB」，或把 .epub 文件拖到窗口里。</p>
<p>← / →、空格翻页，右上角目录可跳到任意章节。</p>
</main></body></html>`;
}

export function BookView(props: BookViewProps) {
  return (
    <WebView
      ref={(node) => props.onView(node)}
      html={props.html || welcomeHtml()}
      scrollable={false}
      style={{ flex: 1 }}
      onLoad={() => props.onLoad()}
      onMessage={(e) => props.onEvent(e.data)}
      onError={(e) => props.onError(e.message)}
    />
  );
}

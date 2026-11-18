//! 组合根：铺满窗口，把拖放这类 surface 级输入接到阅读页。
//!
//! 页面与领域状态都在 `pages/` 与 `store.ts`，这里不写页面逻辑。

import { mount, token, View } from "nsh";
import { Application } from "nsh/desktop";
import { Reader } from "./pages/Reader";
import { openPath } from "./store";

// Finder 的“打开方式”、双击 .epub、拖到 Dock 图标，都由这里接住。
// 在 mount 之前注册：从文件启动的那一份也没有监听者漏掉。
Application.on("openFiles", ({ paths }) => {
  const path = paths[0];
  if (path) void openPath(path);
});

function App() {
  return (
    <View
      style={{ flex: 1, flexDirection: "row", backgroundColor: token("background") }}
      onFileDrop={(e) => {
        const uri = e.uris[0];
        if (uri) void openPath(uri);
      }}
    >
      <Reader />
    </View>
  );
}

mount(App);

//! 组合根：铺满窗口，把拖放这类 surface 级输入接到阅读页。
//!
//! 页面与领域状态都在 `pages/` 与 `store.ts`，这里不写页面逻辑。

import { mount, token, View } from "nsh";
import { Reader } from "./pages/Reader";
import { openPath } from "./store";

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

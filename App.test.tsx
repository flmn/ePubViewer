//! 冒烟测试：应用能挂载，没有打开书时顶栏给出状态与入口。

import { expect, open, test } from "nsh/test";

test("没有书时显示占位标题与打开入口", async () => {
  const app = await open("./App.tsx");
  expect(app.texts()).toContain("尚未打开文件");
  expect(app.texts()).toContain("打开 EPUB");
  // 顶栏的目录与外观入口一直在。
  expect(app.texts()).toContain("目录");
  expect(app.texts()).toContain("Aa");
});

//! 冒烟测试：应用能挂载，没有打开书时顶栏给出状态，底栏给出页码。

import { expect, open, test } from "nsh/test";

test("没有书时显示占位标题与页码", async () => {
  const app = await open("./App.tsx");
  expect(app.texts()).toContain("尚未打开文件");
  // 没有书时全书只有一页。
  expect(app.texts()).toContain("1 / 1");
});

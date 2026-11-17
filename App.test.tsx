//! 冒烟测试：应用能挂载，没有打开书时顶栏给出状态，底栏不出现。

import { expect, open, test } from "nsh/test";

test("没有书时显示占位标题且不显示底栏", async () => {
  const app = await open("./App.tsx");
  expect(app.texts()).toContain("尚未打开文件");
  // 底栏只在打开书之后挂上来，没有书时连页码都没有。
  expect(app.texts()).not.toContain("1 / 1");
});

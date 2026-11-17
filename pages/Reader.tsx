//! 阅读页：顶栏、阅读区、底栏、目录与外观设置；翻页快捷键也归这一页。
//!
//! 阅读进度与设置由 `store.ts` 拥有，这里只读 signal、调用它的命令。

import { effect, keymap, Text, token, View } from "nsh";
import { titleBar } from "nsh/desktop";
import openIcon from "../assets/icons/folder.svg";
import listIcon from "../assets/icons/list.svg";
import settingsIcon from "../assets/icons/text-size.svg";
import previousIcon from "../assets/icons/chevron-left.svg";
import nextIcon from "../assets/icons/chevron-right.svg";
import { BookView } from "../components/BookView";
import { IconButton } from "../components/IconButton";
import { SettingsPanel } from "../components/SettingsPanel";
import { TocPanel } from "../components/TocPanel";
import {
  adjustFont,
  applyDocumentTheme,
  attachViewer,
  book,
  busy,
  chapter,
  chapterIndex,
  colorMode,
  doc,
  error,
  fontDefault,
  fontSize,
  jump,
  loadDocument,
  loadSettings,
  onEvent,
  openBook,
  page,
  resetFont,
  setColorMode,
  settingsOpen,
  toc,
  tocOpen,
  toggleSettings,
  toggleToc,
  totalPages,
  turn,
} from "../store";

export function Reader() {
  void loadSettings();

  // 外观或字号变了就把新值推给阅读文档，页面会重新分页并回报页数。
  effect(() => {
    applyDocumentTheme();
  });

  keymap.bind(
    {
      left: () => turn(-1),
      right: () => turn(1),
      space: () => turn(1),
      pageup: () => turn(-1),
      pagedown: () => turn(1),
      "mod+o": () => void openBook(),
      "mod+t": () => toggleToc(),
    },
    { when: "always" },
  );

  return (
    <View style={{ flex: 1, flexDirection: "column" }}>
      <View
        // 系统标题栏隐藏了：这条顶栏同时是窗口的拖动区（双击最大化），
        // 左侧留白按系统窗口控件（macOS 的三个圆点）的实际宽度算。
        desktop={{ windowDrag: true }}
        style={{
          height: 52,
          flexDirection: "row",
          alignItems: "center",
          gap: 10,
          // titleBar().left 是系统窗口控件占到的位置（已含 8pt 余量），
          // 没隐藏标题栏或全屏时它是 0，就退回普通左边距。
          paddingLeft: Math.max(18, titleBar().left + 10),
          paddingRight: 12,
          borderBottomWidth: 1,
          borderColor: token("border"),
          backgroundColor: token("muted"),
        }}
      >
        <Text
          style={{ flex: 1, fontSize: 14, color: token("foreground"), numberOfLines: 1 }}
        >
          {book()?.title ?? "尚未打开文件"}
        </Text>
        {error() ? (
          <Text style={{ fontSize: 12, color: token("destructive"), numberOfLines: 1 }}>
            {error()}
          </Text>
        ) : null}
        <Text style={{ fontSize: 12, color: token("muted-foreground"), numberOfLines: 1 }}>
          {chapter()?.title ?? ""}
        </Text>
        <IconButton
          icon={openIcon}
          label={busy() ? "正在打开" : "打开 EPUB"}
          size={34}
          onPress={() => void openBook()}
        />
        <IconButton
          icon={listIcon}
          label="目录"
          size={34}
          selected={tocOpen()}
          onPress={() => toggleToc()}
        />
        <IconButton
          icon={settingsIcon}
          label="外观与字号"
          size={34}
          selected={settingsOpen()}
          onPress={() => toggleSettings()}
        />
      </View>

      <View style={{ flex: 1, padding: 8 }}>
        {/* 一页纸：圆角裁掉文档自带的底色，投影把它从窗口底上托起来 */}
        <View
          style={{
            flex: 1,
            backgroundColor: token("card"),
            borderRadius: token("radius"),
            overflow: "hidden",
            boxShadow: [{ x: 0, y: 6, blur: 18, color: "#0000001a" }],
          }}
        >
          <BookView
            html={doc().html}
            onView={(view) => attachViewer(view)}
            onLoad={() => loadDocument()}
            onEvent={(data) => onEvent(data)}
            onError={(message) => error.set(message)}
          />
          {/* 左右两个窄条当作翻页热区，覆盖在页面之上 */}
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
          height: 44,
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "center",
          gap: 6,
          borderTopWidth: 1,
          borderColor: token("border"),
          backgroundColor: token("muted"),
        }}
      >
        <IconButton
          icon={previousIcon}
          label="上一页"
          size={34}
          // 左右各放宽 8pt：鼠标压到按钮边上也照样翻页。
          hitSlop={{ left: 8, right: 8, top: 2, bottom: 2 }}
          onPress={() => turn(-1)}
        />
        {/* 等宽数字 + 固定宽度：页数从 9 跳到 10 时页码不会左右晃 */}
        <Text
          style={{
            minWidth: 72,
            textAlign: "center",
            fontSize: 12,
            color: token("muted-foreground"),
            fontVariant: ["tabular-nums"],
          }}
        >
          {`${page() + 1} / ${totalPages()}`}
        </Text>
        <IconButton
          icon={nextIcon}
          label="下一页"
          size={34}
          hitSlop={{ left: 8, right: 8, top: 2, bottom: 2 }}
          onPress={() => turn(1)}
        />
      </View>

      {tocOpen() ? (
        <TocPanel
          toc={toc()}
          chapter={chapterIndex()}
          onJump={jump}
          onClose={() => tocOpen.set(false)}
        />
      ) : null}

      {settingsOpen() ? (
        <SettingsPanel
          mode={colorMode()}
          fontSize={fontSize()}
          fontDefault={fontDefault()}
          onMode={setColorMode}
          onFont={adjustFont}
          onReset={resetFont}
          onClose={() => settingsOpen.set(false)}
        />
      ) : null}
    </View>
  );
}

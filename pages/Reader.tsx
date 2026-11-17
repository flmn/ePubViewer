//! 阅读页：顶栏、阅读区、底栏、目录与外观设置；翻页快捷键也归这一页。
//!
//! 阅读进度与设置由 `store.ts` 拥有，这里只读 signal、调用它的命令。

import { effect, keymap, signal, Text, token, View } from "nsh";
import { Collapsible } from "nsh/base";
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
  spread,
  toc,
  tocOpen,
  toggleSettings,
  toggleToc,
  totalPages,
  turn,
} from "../store";

/** 阅读区四周的留白（points）。 */
const PAD = 22;
/** 对开时两页之间的中缝（points），与页内脚本的 `gap` 保持一致。 */
const GUTTER = 96;
/** 单页的宽高比：0.7 接近大32开，字行不至于太长。 */
const PAGE_RATIO = 0.7;
/** 阅读区宽高比到这个值就并排摆两页（窗口最大化、全屏都会命中）。 */
const SPREAD_RATIO = 1.4;
/** 底栏高度（points）。 */
const BAR_HEIGHT = 44;

/** 底栏内容：上一页 / 页码 / 下一页。 */
function BottomBar() {
  /** 页码：对开是一屏两页（`3-4 / 356`），单页是一页（`3 / 356`）。
   *  注意要在 JSX 里调用它，页码才会跟着 signal 走。 */
  function pageLabel(): string {
    const first = page() + 1;
    const last = Math.min(first + (spread() ? 1 : 0), totalPages());
    return last > first ? `${first}-${last} / ${totalPages()}` : `${first} / ${totalPages()}`;
  }

  return (
    <View
      style={{
        height: BAR_HEIGHT,
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
          minWidth: 96,
          textAlign: "center",
          fontSize: 12,
          color: token("muted-foreground"),
          fontVariant: ["tabular-nums"],
        }}
      >
        {pageLabel()}
      </Text>
      <IconButton
        icon={nextIcon}
        label="下一页"
        size={34}
        hitSlop={{ left: 8, right: 8, top: 2, bottom: 2 }}
        onPress={() => turn(1)}
      />
    </View>
  );
}

export function Reader() {
  void loadSettings();

  // 阅读区实测尺寸：纸张宽度按它算，窗口怎么缩放都保持纸张比例。
  const area = signal({ width: 0, height: 0 });

  /** 单页纸的宽度：由可用高度定，窄窗口时让位给可用宽度。 */
  function pageWidth(): number {
    const pages = spread() ? 2 : 1;
    const innerW = area().width - PAD * 2;
    const innerH = area().height - PAD * 2;
    const cap = pages === 2 ? (innerW - GUTTER) / 2 : innerW;
    // 还没量到尺寸时先用一组保守的默认值，首帧不至于挤成一条。
    const ideal = innerH > 0 ? innerH * PAGE_RATIO : 620;
    return Math.round(Math.max(240, Math.min(ideal, cap > 0 ? cap : ideal)));
  }

  /** 纸张总宽度：对开是两页加中缝。 */
  function paperWidth(): number {
    return spread() ? pageWidth() * 2 + GUTTER : pageWidth();
  }

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

      <View
        style={{
          flex: 1,
          padding: PAD,
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "center",
        }}
        onLayout={(e) => {
          const size = e.bounds.size;
          if (size.width !== area().width || size.height !== area().height) {
            area.set(size);
          }
          // 宽高比够大就摊开成两页；窗口缩回去自动收回单页。
          spread.set(size.height > 0 && size.width >= size.height * SPREAD_RATIO);
        }}
      >
        {/* 一页纸（对开时是两页）：宽度按纸张比例算，居中摆在窗口里；
            圆角裁掉文档自带的底色，投影把它从窗口底上托起来 */}
        <View
          style={{
            width: paperWidth(),
            height: "100%",
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
          {/* 对开时画一道书脊，让两页看起来是一本书摊开 */}
          {spread() ? (
            <View
              style={{
                position: "absolute",
                left: "50%",
                top: 18,
                bottom: 18,
                width: 1,
                backgroundColor: token("border"),
                opacity: 0.7,
              }}
            />
          ) : null}
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

      {/* 底栏：没打开书时高度为零，不占位置；打开时高度从 0 长到 BAR_HEIGHT，
          把它上面的阅读区一起顶上去，栏本身就像从窗口下沿推出来 */}
      <Collapsible open={!!book()} reveal content={<BottomBar />} />

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

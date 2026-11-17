//! 阅读页：顶栏、阅读区、底栏、目录与外观设置；翻页快捷键也归这一页。
//!
//! 阅读进度与设置由 `store.ts` 拥有，这里只读 signal、调用它的命令。

import { effect, keymap, Text, token, View } from "nsh";
import { Button } from "nsh/base";
import { BookView } from "../components/BookView";
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
        style={{
          height: 52,
          flexDirection: "row",
          alignItems: "center",
          gap: 12,
          paddingLeft: 18,
          paddingRight: 18,
          borderBottomWidth: 1,
          borderColor: token("border"),
          backgroundColor: token("muted"),
        }}
      >
        <Button
          onPress={() => void openBook()}
          style={{
            paddingLeft: 14,
            paddingRight: 14,
            paddingTop: 8,
            paddingBottom: 8,
            borderRadius: token("radius"),
            backgroundColor: token("primary"),
          }}
        >
          <Text style={{ fontSize: 13, color: token("primary-foreground") }}>
            {busy() ? "打开中…" : "打开 EPUB"}
          </Text>
        </Button>
        <Text style={{ flex: 1, fontSize: 14, color: token("foreground") }}>
          {book() ? book()!.title : "尚未打开文件"}
        </Text>
        {error() ? <Text style={{ fontSize: 12, color: token("destructive") }}>{error()}</Text> : null}
        <Text style={{ fontSize: 12, color: token("muted-foreground") }}>{chapter()?.title ?? ""}</Text>
        <Button
          onPress={() => toggleToc()}
          style={{
            paddingLeft: 12,
            paddingRight: 12,
            paddingTop: 8,
            paddingBottom: 8,
            borderRadius: token("radius"),
            backgroundColor: tocOpen() ? token("secondary") : undefined,
          }}
        >
          <Text style={{ fontSize: 13, color: token("muted-foreground") }}>目录</Text>
        </Button>
        <Button
          onPress={() => toggleSettings()}
          style={{
            paddingLeft: 12,
            paddingRight: 12,
            paddingTop: 8,
            paddingBottom: 8,
            borderRadius: token("radius"),
            backgroundColor: settingsOpen() ? token("secondary") : undefined,
          }}
        >
          <Text style={{ fontSize: 13, color: token("muted-foreground") }}>Aa</Text>
        </Button>
      </View>

      <View style={{ flex: 1, padding: 22 }}>
        <View style={{ flex: 1, backgroundColor: token("card"), borderRadius: token("radius") }}>
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
          height: 48,
          flexDirection: "row",
          alignItems: "center",
          justifyContent: "center",
          gap: 18,
          borderTopWidth: 1,
          borderColor: token("border"),
          backgroundColor: token("muted"),
        }}
      >
        <Button onPress={() => turn(-1)} style={{ padding: 8 }}>
          <Text style={{ fontSize: 13, color: token("muted-foreground") }}>‹ 上一页</Text>
        </Button>
        <Text style={{ fontSize: 13, color: token("foreground") }}>{`${page() + 1} / ${totalPages()}`}</Text>
        <Button onPress={() => turn(1)} style={{ padding: 8 }}>
          <Text style={{ fontSize: 13, color: token("muted-foreground") }}>下一页 ›</Text>
        </Button>
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

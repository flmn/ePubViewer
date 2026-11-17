//! 目录浮层：列出全书目录，点条目跳转，点空白处关闭。

import { List, Text, token, View } from "nsh";
import { Button } from "nsh/base";
import type { TocEntry } from "epubviewer/native";

export type TocPanelProps = {
  /** 全书目录，按层级排好。 */
  toc: TocEntry[];
  /** 当前所在章，用来高亮。 */
  chapter: number;
  /** 点了某个目录项。 */
  onJump(entry: TocEntry): void;
  /** 关了浮层。 */
  onClose(): void;
};

export function TocPanel(props: TocPanelProps) {
  return (
    <View style={{ position: "absolute", left: 0, right: 0, top: 0, bottom: 0 }}>
      <View
        style={{ position: "absolute", left: 0, right: 0, top: 0, bottom: 0 }}
        onPress={() => props.onClose()}
      />
      <View
        style={{
          position: "absolute",
          top: 58,
          right: 12,
          width: 320,
          height: 600,
          flexDirection: "column",
          backgroundColor: token("popover"),
          borderRadius: token("radius"),
          borderWidth: 1,
          borderColor: token("border"),
          overflow: "hidden",
        }}
      >
        <View
          style={{
            paddingLeft: 16,
            paddingRight: 16,
            paddingTop: 12,
            paddingBottom: 6,
            flexDirection: "row",
          }}
        >
          <Text style={{ flex: 1, fontSize: 12, color: token("muted-foreground") }}>目录</Text>
          <Text style={{ fontSize: 12, color: token("muted-foreground") }}>
            {`${props.toc.length} 项`}
          </Text>
        </View>
        <List
          each={props.toc}
          style={{ flex: 1, paddingBottom: 8 }}
          empty={
            <View style={{ padding: 16 }}>
              <Text style={{ fontSize: 13, color: token("muted-foreground") }}>
                这本书没有目录条目
              </Text>
            </View>
          }
        >
          {(item) => (
            <Button
              onPress={() => props.onJump(item())}
              style={{
                paddingLeft: 16 + item().level * 14,
                paddingRight: 16,
                paddingTop: 7,
                paddingBottom: 7,
                backgroundColor:
                  item().chapter === props.chapter ? token("secondary") : undefined,
              }}
            >
              <Text
                style={{
                  fontSize: 13,
                  fontWeight: item().chapter === props.chapter ? "bold" : "normal",
                  color:
                    item().chapter === props.chapter
                      ? token("foreground")
                      : token("muted-foreground"),
                }}
              >
                {item().label}
              </Text>
            </Button>
          )}
        </List>
      </View>
    </View>
  );
}

//! 外观设置：切换颜色模式、调整正文字号。

import { Text, token, View } from "nsh";
import { Button } from "nsh/base";
import type { ColorMode } from "../store";

export type SettingsPanelProps = {
  /** 当前颜色模式，用来高亮。 */
  mode: ColorMode;
  /** 正文字号，单位 pt。 */
  fontSize: number;
  /** 字号就是默认值，用来把「重置」画淡。 */
  fontDefault: boolean;
  /** 选了颜色模式。 */
  onMode(mode: ColorMode): void;
  /** 调了字号，`delta` 为步长。 */
  onFont(delta: number): void;
  /** 字号回到默认值。 */
  onReset(): void;
  /** 关了面板。 */
  onClose(): void;
};

/** 分段按钮的通用外观；`selected` 时用次要色底。 */
function segment(selected: boolean) {
  return {
    paddingLeft: 14,
    paddingRight: 14,
    paddingTop: 7,
    paddingBottom: 7,
    borderRadius: token("radius"),
    backgroundColor: selected ? token("secondary") : token("background"),
  };
}

function segmentText(selected: boolean): {
  fontSize: number;
  fontWeight: "bold" | "normal";
  color: string;
} {
  return {
    fontSize: 13,
    fontWeight: selected ? "bold" : "normal",
    color: selected ? token("foreground") : token("muted-foreground"),
  };
}

/** 字号加减按钮：固定等宽，中间的数字才落在正中。 */
function step(): {
  width: number;
  height: number;
  alignItems: "center";
  justifyContent: "center";
  borderRadius: number;
  backgroundColor: string;
} {
  return {
    width: 40,
    height: 32,
    alignItems: "center",
    justifyContent: "center",
    borderRadius: token("radius"),
    backgroundColor: token("background"),
  };
}

function stepText(): { fontSize: number; color: string } {
  return { fontSize: 13, color: token("muted-foreground") };
}

export function SettingsPanel(props: SettingsPanelProps) {
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
          width: 264,
          flexDirection: "column",
          gap: 12,
          padding: 16,
          backgroundColor: token("popover"),
          borderRadius: token("radius"),
          borderWidth: 1,
          borderColor: token("border"),
        }}
      >
        <Text style={{ fontSize: 12, color: token("muted-foreground") }}>颜色模式</Text>
        <View style={{ flexDirection: "row", gap: 8 }}>
          <Button
            onPress={() => props.onMode("light")}
            style={{ ...segment(props.mode === "light") }}
          >
            <Text style={segmentText(props.mode === "light")}>浅色</Text>
          </Button>
          <Button onPress={() => props.onMode("dark")} style={{ ...segment(props.mode === "dark") }}>
            <Text style={segmentText(props.mode === "dark")}>深色</Text>
          </Button>
          <Button
            onPress={() => props.onMode("system")}
            style={{ ...segment(props.mode === "system") }}
          >
            <Text style={segmentText(props.mode === "system")}>跟随系统</Text>
          </Button>
        </View>

        <View style={{ flexDirection: "row", alignItems: "center", justifyContent: "space-between" }}>
          <Text style={{ fontSize: 12, color: token("muted-foreground") }}>字号</Text>
          <Button
            onPress={() => props.onReset()}
            style={{
              paddingLeft: 8,
              paddingRight: 8,
              paddingTop: 4,
              paddingBottom: 4,
              borderRadius: token("radius"),
              opacity: props.fontDefault ? 0.4 : 1,
            }}
          >
            <Text style={{ fontSize: 12, color: token("muted-foreground") }}>重置</Text>
          </Button>
        </View>
        <View style={{ flexDirection: "row", alignItems: "center", gap: 10 }}>
          <Button onPress={() => props.onFont(-1)} style={step()}>
            <Text style={stepText()}>A−</Text>
          </Button>
          <Text
            style={{
              flex: 1,
              textAlign: "center",
              fontSize: 13,
              color: token("foreground"),
            }}
          >
            {`${props.fontSize} pt`}
          </Text>
          <Button onPress={() => props.onFont(1)} style={step()}>
            <Text style={stepText()}>A+</Text>
          </Button>
        </View>
      </View>
    </View>
  );
}

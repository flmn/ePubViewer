//! 只有图标的按钮：图标自己不带文字，名称交给无障碍标签。
//!
//! 悬停与按下都给出底色反馈——图标按钮没有文字，光靠图标本身看不出
//! 可点范围有多大。

import { Image, token } from "nsh";
import type { Edges, ImageAsset } from "nsh";
import { Button } from "nsh/base";

export type IconButtonProps = {
  /** 图标的 SVG 资源（导入的静态资源）。 */
  icon: ImageAsset;
  /** 无障碍名称；图标没有文字，读屏靠它。 */
  label: string;
  /** 处于打开/选中状态，用次要色底。 */
  selected?: boolean;
  /** 按钮的边长（points，正方形），图标约为它的 56%。 */
  size?: number;
  /** 按下范围在按钮外扩出多少 points；缺省四周 2pt。 */
  hitSlop?: number | Partial<Edges>;
  /** 点了。 */
  onPress(): void;
};

export function IconButton(props: IconButtonProps) {
  const size = props.size ?? 32;

  return (
    <Button
      onPress={() => props.onPress()}
      accessibilityLabel={props.label}
      selected={props.selected}
      // 图标小，按下范围往外放宽一点，不用正中靶心。
      hitSlop={props.hitSlop ?? 2}
      style={{
        width: size,
        height: size,
        alignItems: "center",
        justifyContent: "center",
        borderRadius: token("radius"),
      }}
      selectedStyle={{ backgroundColor: token("secondary") }}
      // 悬停：底色浮出来，等于把可点区域画给用户看。
      hoverStyle={{ backgroundColor: token("secondary") }}
      // 按下：再压暗一层，和悬停分得开。
      activeStyle={{ backgroundColor: token("accent"), opacity: 0.75 }}
      focusVisibleStyle={{
        outlineWidth: 2,
        outlineColor: token("ring"),
        outlineOffset: 1,
      }}
    >
      <Image
        src={props.icon}
        style={{
          width: Math.round(size * 0.56),
          height: Math.round(size * 0.56),
          tintColor: props.selected ? token("foreground") : token("muted-foreground"),
        }}
      />
    </Button>
  );
}

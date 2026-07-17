//! 布局段(挂在 `TuiConfig` 下):完整布局门槛 + 全屏分区尺寸 + 浮层 dock 宽。

use mineral_config_macros::config_section;
use std::fmt;

use num_traits::ToPrimitive;
use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};

/// 布局配置。
#[config_section]
pub struct LayoutConfig {
    /// 启用完整布局的最小终端宽(列);不足走紧凑布局(无歌词 / 频谱面板)。
    min_full_width: u16,

    /// 启用完整布局的最小终端高(行);不足走紧凑布局。
    min_full_height: u16,

    /// 全屏态左栏(封面 + transport)占宽百分比(0-100),余下归歌词。
    fs_left_pct: u16,

    /// 全屏态底部频谱通栏高(行)。
    fs_spectrum_height: u16,

    /// 全屏态 transport 区高(行);内容 6 行 + 边框 2。
    fs_transport_height: u16,

    /// 停靠浮层(播放队列)dock 宽占屏宽百分比(0-100)。
    dock_w_pct: u16,

    /// 锚定弹出菜单(PopMenu)相对锚点行的横向对齐。
    menu_align: MenuAlign,
}

/// 锚定弹出菜单相对锚点行的横向对齐。不依赖渲染 crate;接线处经 [`Self::permille`] 消费。
///
/// 锚点行通常横跨整个左栏,菜单本身窄得多——对齐决定菜单落在行的哪一段。配置可写
/// 关键字 `"left"`/`"center"`/`"right"`,或一个 `0.0..=1.0` 的数字精确指定比例
/// (`0` = 贴左、`0.5` = 居中、`1` = 贴右)。
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum MenuAlign {
    /// 贴锚点行左缘(菜单左对齐),等价比例 `0`。
    Left,

    /// 居锚点行正中(菜单居中),等价比例 `0.5`。
    Center,

    /// 贴锚点行右缘(菜单右对齐),等价比例 `1`。
    Right,

    /// 精确比例:菜单可移动跨度内的归一化位置,`0.0` 贴左 ~ `1.0` 贴右。
    Fraction(f64),
}

impl MenuAlign {
    /// 归一化位置的千分比定点(`0..=1000`),供渲染层做整数对齐插值。
    ///
    /// # Return:
    ///   `Left` = 0、`Center` = 500、`Right` = 1000、`Fraction(f)` = `clamp(0,1) × 1000` 四舍五入。
    pub fn permille(self) -> u32 {
        match self {
            Self::Left => 0,
            Self::Center => 500,
            Self::Right => 1000,
            Self::Fraction(f) => (f.clamp(0.0, 1.0) * 1000.0).round().to_u32().unwrap_or(500),
        }
    }
}

impl<'de> Deserialize<'de> for MenuAlign {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// 接受关键字字符串或 `0.0..=1.0` 数字两种写法。
        struct AlignVisitor;

        impl Visitor<'_> for AlignVisitor {
            type Value = MenuAlign;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(r#""left" / "center" / "right",或 0.0..=1.0 的数字"#)
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<MenuAlign, E> {
                match s {
                    "left" => Ok(MenuAlign::Left),
                    "center" => Ok(MenuAlign::Center),
                    "right" => Ok(MenuAlign::Right),
                    other => Err(E::unknown_variant(other, &["left", "center", "right"])),
                }
            }

            fn visit_f64<E: de::Error>(self, v: f64) -> Result<MenuAlign, E> {
                Ok(MenuAlign::Fraction(v))
            }

            fn visit_u64<E: de::Error>(self, v: u64) -> Result<MenuAlign, E> {
                Ok(MenuAlign::Fraction(v.to_f64().unwrap_or(0.5)))
            }

            fn visit_i64<E: de::Error>(self, v: i64) -> Result<MenuAlign, E> {
                Ok(MenuAlign::Fraction(v.to_f64().unwrap_or(0.5)))
            }
        }

        deserializer.deserialize_any(AlignVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::MenuAlign;

    /// 从 JSON 值解析 `MenuAlign`(模拟 Lua → serde_json → 落型的真实路径)。
    fn parse(v: serde_json::Value) -> color_eyre::Result<MenuAlign> {
        Ok(serde_json::from_value::<MenuAlign>(v)?)
    }

    /// 关键字三档映射到固定千分比。
    #[test]
    fn keywords_map_to_endpoints() -> color_eyre::Result<()> {
        assert_eq!(parse(serde_json::json!("left"))?.permille(), 0);
        assert_eq!(parse(serde_json::json!("center"))?.permille(), 500);
        assert_eq!(parse(serde_json::json!("right"))?.permille(), 1000);
        Ok(())
    }

    /// 小数比例按 × 1000 四舍五入成千分比;整数 0 / 1 同样接受。
    #[test]
    fn number_maps_to_permille() -> color_eyre::Result<()> {
        assert_eq!(parse(serde_json::json!(0.25))?.permille(), 250);
        assert_eq!(parse(serde_json::json!(0.333))?.permille(), 333);
        assert_eq!(parse(serde_json::json!(0))?.permille(), 0);
        assert_eq!(parse(serde_json::json!(1))?.permille(), 1000);
        Ok(())
    }

    /// 越界比例被钳到 `[0, 1]`。
    #[test]
    fn out_of_range_fraction_clamps() -> color_eyre::Result<()> {
        assert_eq!(parse(serde_json::json!(1.5))?.permille(), 1000);
        assert_eq!(parse(serde_json::json!(-0.2))?.permille(), 0);
        Ok(())
    }

    /// 非法关键字报错(不静默吞掉)。
    #[test]
    fn unknown_keyword_errors() {
        assert!(parse(serde_json::json!("diagonal")).is_err());
    }
}

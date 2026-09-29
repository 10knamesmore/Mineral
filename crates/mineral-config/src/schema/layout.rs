//! 布局段(挂在 `TuiConfig` 下):完整布局门槛、共用播放栏高度、全屏分区与浮层宽度。

use mineral_config_macros::config_section;
use std::fmt;

use num_traits::ToPrimitive;
use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};

/// 布局配置。
#[config_section]
pub struct LayoutConfig {
    /// 完整布局最小列数
    min_full_width: u16,

    /// 完整布局最小行数
    min_full_height: u16,

    /// 全屏左栏宽度百分比
    fs_left_pct: u16,

    /// 全屏频谱高度
    fs_spectrum: FsSpectrumConfig,

    /// 播放栏行数，含边框
    transport_height: u16,

    /// 停靠浮层宽度百分比
    dock_w_pct: u16,

    /// 菜单横向对齐
    menu_align: MenuAlign,
}

/// 频谱按屏高百分比计算，再限制行数
#[config_section]
pub struct FsSpectrumConfig {
    /// 占屏幕高度的百分比
    pct: u16,

    /// 最少行数
    min: u16,

    /// 最多行数
    max: u16,
}

impl FsSpectrumConfig {
    /// 给定终端总高,解出频谱通栏应占的行数。
    ///
    /// # Params:
    ///   - `total_height`: 全屏 area 的总行数
    ///
    /// # Return:
    ///   `total_height × pct%` 钳到 `[min, max]`,且不超过 `total_height` 本身。
    pub fn resolve(&self, total_height: u16) -> u16 {
        let pct = u32::from(self.pct.min(100));
        let scaled = u16::try_from(u32::from(total_height) * pct / 100).unwrap_or(u16::MAX);
        let floor = self.min;
        // 容错坏配置(min > max):上限至少不低于下限,避免 clamp panic。
        let ceil = self.max.max(floor);
        scaled.clamp(floor, ceil).min(total_height)
    }
}

/// 菜单横向对齐：left、center、right 或 0-1 比例
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum MenuAlign {
    /// 左对齐
    Left,

    /// 居中
    Center,

    /// 右对齐
    Right,

    /// 对齐比例，0 贴左、1 贴右
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
    use super::{FsSpectrumConfig, MenuAlign};

    /// 从 JSON 值落型出 `FsSpectrumConfig`(模拟 Lua → serde_json → 落型路径)。
    fn spectrum(pct: u16, min: u16, max: u16) -> color_eyre::Result<FsSpectrumConfig> {
        Ok(serde_json::from_value::<FsSpectrumConfig>(
            serde_json::json!({ "pct": pct, "min": min, "max": max }),
        )?)
    }

    /// 中段尺寸走百分比,两端被上下限钳住。
    #[test]
    fn resolve_scales_then_clamps() -> color_eyre::Result<()> {
        let spec = spectrum(/*pct*/ 28, /*min*/ 10, /*max*/ 22)?;
        // 50 行:28% = 14,落在 [10,22] 内,原样取用。
        assert_eq!(spec.resolve(/*total_height*/ 50), 14);
        // 30 行:28% ≈ 8,低于下限 → 提到 10。
        assert_eq!(spec.resolve(/*total_height*/ 30), 10);
        // 100 行:28% = 28,高于上限 → 压到 22。
        assert_eq!(spec.resolve(/*total_height*/ 100), 22);
        Ok(())
    }

    /// 下限比屏还高时不越界:最终不超过总高。
    #[test]
    fn resolve_never_exceeds_total() -> color_eyre::Result<()> {
        let spec = spectrum(/*pct*/ 50, /*min*/ 40, /*max*/ 60)?;
        assert_eq!(spec.resolve(/*total_height*/ 8), 8);
        Ok(())
    }

    /// 坏配置(min > max)不 panic:上限被提到不低于下限。
    #[test]
    fn resolve_tolerates_inverted_bounds() -> color_eyre::Result<()> {
        let spec = spectrum(/*pct*/ 90, /*min*/ 30, /*max*/ 10)?;
        assert_eq!(spec.resolve(/*total_height*/ 100), 30);
        Ok(())
    }

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

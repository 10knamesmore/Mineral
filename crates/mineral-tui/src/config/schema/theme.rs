//! Interface colors, token references, and effects. Color intents resolve after configuration loading.

use std::fmt;

use mineral_config_macros::{config_section, lua_enum};
use serde::de::value::SeqAccessDeserializer;
use serde::de::{Error, IntoDeserializer, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};

pub use mineral_script::{AnsiSlot, ColorMix, ColorSource, ColorValue, HexColor, TerminalColor};

/// Palette token names accepted by color references.
const TOKEN_NAMES: [&str; 14] = [
    "base", "mantle", "crust", "surface0", "surface1", "overlay", "subtext", "text", "accent",
    "accent_2", "red", "yellow", "green", "peach",
];

/// Interface colors and effects. Palette colors cannot reference other palette tokens.
#[config_section]
pub struct ThemeConfig {
    /// Screen background; "reset" preserves the terminal's default background.
    background: ColorRef,

    /// Startup color query deadline in milliseconds; changes apply on the next launch.
    query_timeout_ms: u64,

    /// Main background.
    base: ColorValue,

    /// Panel and popup background.
    mantle: ColorValue,

    /// Transport and command background.
    crust: ColorValue,

    /// Selected row background and progress track.
    surface0: ColorValue,

    /// Unfocused borders and separators.
    surface1: ColorValue,

    /// Muted text.
    overlay: ColorValue,

    /// Secondary text and metadata.
    subtext: ColorValue,

    /// Primary text.
    text: ColorValue,

    /// Primary accent.
    accent: ColorValue,

    /// Secondary accent.
    accent_2: ColorValue,

    /// Errors, removal, and favorite markers.
    red: ColorValue,

    /// Pause indicator.
    yellow: ColorValue,

    /// Playback indicator.
    green: ColorValue,

    /// Command and search prefixes.
    peach: ColorValue,

    /// Search match styling.
    search_hit: SearchHitConfig,

    /// Cover-driven accents.
    dynamic: DynamicThemeConfig,

    /// Text opacity levels.
    text_alpha: TextAlphaConfig,
}

impl ThemeConfig {
    pub(crate) fn colors(&self) -> [&ColorValue; 14] {
        [
            &self.base,
            &self.mantle,
            &self.crust,
            &self.surface0,
            &self.surface1,
            &self.overlay,
            &self.subtext,
            &self.text,
            &self.accent,
            &self.accent_2,
            &self.red,
            &self.yellow,
            &self.green,
            &self.peach,
        ]
    }
}

/// 封面驱动强调色，取色失败用静态配色
#[config_section]
pub struct DynamicThemeConfig {
    /// 是否启用
    enabled: bool,

    /// 强调色过渡时长
    fade_ms: u32,

    /// Minimum WCAG contrast for cover accents, from 1 through 21.
    #[serde(deserialize_with = "deserialize_contrast")]
    minimum_contrast: f32,
}

fn deserialize_contrast<'de, D>(deserializer: D) -> Result<f32, D::Error>
where
    D: Deserializer<'de>,
{
    let contrast = f32::deserialize(deserializer)?;
    if !contrast.is_finite() || !(1.0..=21.0).contains(&contrast) {
        return Err(Error::custom(
            "minimum contrast must be a finite number from 1 through 21",
        ));
    }
    Ok(contrast)
}

/// 文本不透明度 0-1；背景非真彩时用静态配色
#[config_section]
pub struct TextAlphaConfig {
    /// 次级文本不透明度
    strong: f32,

    /// 暗淡标签不透明度
    muted: f32,

    /// 弱线条不透明度
    faint: f32,

    /// 近背景元素不透明度
    ghost: f32,
}

/// 搜索命中样式
#[config_section]
pub struct SearchHitConfig {
    /// 高亮色
    color: ColorRef,

    /// 叠加字体效果；空数组仅变色
    modifiers: Vec<TextStyle>,
}

/// A color intent, a fixed color, or a theme token name such as "peach".
#[derive(Clone, Debug, PartialEq)]
pub enum ColorRef {
    Value(ColorValue),

    Token(TokenName),
}

impl<'de> Deserialize<'de> for ColorRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ColorRefVisitor)
    }
}

struct ColorRefVisitor;

impl<'de> Visitor<'de> for ColorRefVisitor {
    type Value = ColorRef;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a color intent, a #rrggbb string, reset, or a theme token name")
    }

    fn visit_str<E>(self, value: &str) -> Result<ColorRef, E>
    where
        E: Error,
    {
        if value.starts_with('#') || value == "reset" {
            ColorValue::deserialize(value.into_deserializer()).map(ColorRef::Value)
        } else {
            TokenName::deserialize(value.into_deserializer()).map(ColorRef::Token)
        }
    }

    fn visit_seq<A>(self, sequence: A) -> Result<ColorRef, A::Error>
    where
        A: SeqAccess<'de>,
    {
        ColorValue::deserialize(SeqAccessDeserializer::new(sequence)).map(ColorRef::Value)
    }
}

/// 字体效果，需终端支持
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    /// 加粗。
    Bold,

    /// 斜体。
    Italic,

    /// 下划线。
    Underline,

    /// 暗淡。
    Dim,

    /// 反色(前景背景互换)。
    Reversed,

    /// 删除线。
    CrossedOut,
}

/// A validated name from the theme palette.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenName {
    name: String,
}

impl TokenName {
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl<'de> Deserialize<'de> for TokenName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        if TOKEN_NAMES.contains(&raw.as_str()) {
            Ok(Self { name: raw })
        } else {
            Err(Error::custom(format!(
                "unknown theme token `{raw}`; expected one of {TOKEN_NAMES:?}"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AnsiSlot, ColorRef, ColorValue, HexColor, TextStyle, TokenName};

    #[test]
    fn dynamic_override_takes_effect() -> color_eyre::Result<()> {
        let tree = mineral_config::merge_tree(
            crate::config::default_tui_tree()?,
            serde_json::json!({ "theme": {
                "dynamic": { "enabled": false, "fade_ms": 500 },
            } }),
        );
        let cfg = crate::config::tui_from_tree(&tree)
            .map_err(|w| color_eyre::eyre::eyre!("invalid override: {w}"))?;
        let d = cfg.theme().dynamic();
        assert!(!*d.enabled());
        assert_eq!(*d.fade_ms(), 500);
        Ok(())
    }

    #[test]
    fn hex_color_parses_valid() -> color_eyre::Result<()> {
        let c: HexColor = serde_json::from_value(serde_json::json!("#1e1e2e"))?;
        assert_eq!((c.r(), c.g(), c.b()), (0x1e, 0x1e, 0x2e));
        Ok(())
    }

    #[test]
    fn hex_color_rejects_invalid() {
        for value in ["1e1e2e", "#gggggg", "#1e1e2"] {
            assert!(serde_json::from_value::<HexColor>(serde_json::json!(value)).is_err());
        }
    }

    #[test]
    fn token_name_validates_membership() -> color_eyre::Result<()> {
        let t: TokenName = serde_json::from_value(serde_json::json!("red"))?;
        assert_eq!(t.as_str(), "red");
        assert!(serde_json::from_value::<TokenName>(serde_json::json!("mauve")).is_err());
        Ok(())
    }

    #[test]
    fn color_value_hex_string() -> color_eyre::Result<()> {
        let value: ColorValue = serde_json::from_value(serde_json::json!("#102030"))?;
        assert!(matches!(value, ColorValue::Hex(c) if (c.r(), c.g(), c.b()) == (0x10, 0x20, 0x30)));
        assert!(serde_json::from_value::<ColorValue>(serde_json::json!("#12345")).is_err());
        Ok(())
    }

    #[test]
    fn color_value_ansi_intent() -> color_eyre::Result<()> {
        let named: ColorValue = serde_json::from_value(serde_json::json!(["ansi", "blue"]))?;
        assert!(matches!(named, ColorValue::Ansi(s) if s.index() == 4));
        let numbered: ColorValue = serde_json::from_value(serde_json::json!(["ansi", 8]))?;
        assert!(matches!(numbered, ColorValue::Ansi(s) if s.index() == 8));
        Ok(())
    }

    #[test]
    fn color_value_reset_string() -> color_eyre::Result<()> {
        let value: ColorValue = serde_json::from_value(serde_json::json!("reset"))?;
        assert!(matches!(value, ColorValue::Reset));
        Ok(())
    }

    #[test]
    fn color_value_rejects_token_and_bad_table() {
        for bad in [
            serde_json::json!("peach"),
            serde_json::json!({ "token": "peach" }),
            serde_json::json!({}),
            serde_json::json!({ "ansi": "blue", "reset": true }),
            serde_json::json!({ "rgb": "102030" }),
            serde_json::json!({ "reset": false }),
        ] {
            assert!(
                serde_json::from_value::<ColorValue>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn color_ref_accepts_token_and_delegates_to_value() -> color_eyre::Result<()> {
        let token: ColorRef = serde_json::from_value(serde_json::json!("peach"))?;
        assert!(matches!(token, ColorRef::Token(ref n) if n.as_str() == "peach"));
        let hex: ColorRef = serde_json::from_value(serde_json::json!("#102030"))?;
        assert!(
            matches!(hex, ColorRef::Value(ColorValue::Hex(c)) if (c.r(), c.g(), c.b()) == (0x10, 0x20, 0x30))
        );
        let ansi: ColorRef = serde_json::from_value(serde_json::json!(["ansi", "blue"]))?;
        assert!(matches!(ansi, ColorRef::Value(ColorValue::Ansi(s)) if s.index() == 4));
        let reset: ColorRef = serde_json::from_value(serde_json::json!("reset"))?;
        assert!(matches!(reset, ColorRef::Value(ColorValue::Reset)));
        assert!(serde_json::from_value::<ColorRef>(serde_json::json!("mauve")).is_err());
        Ok(())
    }

    #[test]
    fn ansi_slot_named_parses_all_sixteen() -> color_eyre::Result<()> {
        let cases = [
            ("black", 0),
            ("red", 1),
            ("green", 2),
            ("yellow", 3),
            ("blue", 4),
            ("magenta", 5),
            ("cyan", 6),
            ("white", 7),
            ("bright_black", 8),
            ("bright_red", 9),
            ("bright_green", 10),
            ("bright_yellow", 11),
            ("bright_blue", 12),
            ("bright_magenta", 13),
            ("bright_cyan", 14),
            ("bright_white", 15),
        ];
        for (name, index) in cases {
            let slot: AnsiSlot = serde_json::from_value(serde_json::json!(name))?;
            assert_eq!(slot.index(), index);
        }
        Ok(())
    }

    #[test]
    fn ansi_slot_numeric_parses() -> color_eyre::Result<()> {
        for index in [0_u8, 4, 15] {
            let slot: AnsiSlot = serde_json::from_value(serde_json::json!(index))?;
            assert_eq!(slot.index(), index);
        }
        Ok(())
    }

    #[test]
    fn ansi_slot_rejects_out_of_range_and_bad_name() {
        assert!(serde_json::from_value::<AnsiSlot>(serde_json::json!(16)).is_err());
        assert!(serde_json::from_value::<AnsiSlot>(serde_json::json!("mauve")).is_err());
    }

    #[test]
    fn text_style_parses_known_rejects_unknown() -> color_eyre::Result<()> {
        let styles: Vec<TextStyle> =
            serde_json::from_value(serde_json::json!(["bold", "italic", "crossed_out"]))?;
        assert_eq!(
            styles,
            vec![TextStyle::Bold, TextStyle::Italic, TextStyle::CrossedOut]
        );
        assert!(serde_json::from_value::<TextStyle>(serde_json::json!("blink")).is_err());
        Ok(())
    }
}

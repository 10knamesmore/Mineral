//! Fixed RGB palette used when required terminal color replies are unavailable.

use mineral_config_macros::config_section;

use super::theme::HexColor;

/// Replaces the terminal palette as a whole; does not accept color expressions.
#[config_section]
pub struct ThemeDefaults {
    /// Default text color, in #rrggbb format.
    #[lua_type("string")]
    foreground: HexColor,

    /// Default background color, in #rrggbb format.
    #[lua_type("string")]
    background: HexColor,

    /// Exactly 16 #rrggbb colors, ordered by ANSI slots 0 through 15.
    #[lua_type("string[]")]
    ansi: [HexColor; 16],
}

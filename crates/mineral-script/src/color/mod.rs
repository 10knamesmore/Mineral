//! Deferred colors shared by Lua configuration values and host rendering.

mod lua;
mod value;

pub use lua::install_color_api;
pub use value::{AnsiSlot, ColorMix, ColorSource, ColorValue, HexColor, TerminalColor};

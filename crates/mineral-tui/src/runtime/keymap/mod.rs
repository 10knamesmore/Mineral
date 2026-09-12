//! 键位查表、终端和弦归一与帮助目录。

mod bindings;
mod chord;
pub mod help;

pub use bindings::Keymap;
pub use chord::chord_from_event;

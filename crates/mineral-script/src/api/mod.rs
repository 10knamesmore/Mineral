//! `mineral.*` Lua API 的边界适配层:Lua 值 ↔ 结构化 Rust 类型的
//! 转换只发生在这里,出了本模块全是强类型。

pub(crate) mod actions;
pub(crate) mod events;
pub(crate) mod observe;
pub(crate) mod player;
pub(crate) mod ui;
pub(crate) mod value;

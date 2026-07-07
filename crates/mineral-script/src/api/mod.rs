//! `mineral.*` Lua API 的边界适配层:Lua 值 ↔ 结构化 Rust 类型的
//! 转换只发生在这里,出了本模块全是强类型。
//!
//! **布局与 Lua API 树一一对应**:顶层函数 = 顶层文件(`on.rs` ↔
//! `mineral.on`),子表 = 目录、子表函数 = 目录内文件(`store/get.rs` ↔
//! `mineral.store.get`)。新增 API 先在这棵树上找到它的位置。

pub(crate) mod action;
pub(crate) mod bind;
pub(crate) mod config;
pub(crate) mod download;
pub(crate) mod emit;
pub(crate) mod get;
pub(crate) mod hook;
pub(crate) mod library;
pub(crate) mod log;
pub(crate) mod observe;
pub(crate) mod on;
pub(crate) mod on_message;
pub(crate) mod player;
pub(crate) mod queue;
pub(crate) mod spawn;
pub(crate) mod store;
pub(crate) mod sys;
pub(crate) mod timer;
pub(crate) mod ui;
pub(crate) mod value;

#[cfg(test)]
pub(crate) mod test_support;

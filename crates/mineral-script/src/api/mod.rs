//! Lua API 的值解析与宿主模块安装。
//!
//! 音乐 API 只由 daemon 安装，界面 API 只由 TUI 安装；
//! sys、log 与配置覆盖解析由两侧共用。

pub(crate) mod config;
pub(crate) mod download;
pub(crate) mod hook;
pub(crate) mod library;
pub(crate) mod log;
pub(crate) mod player;
pub(crate) mod queue;
pub(crate) mod store;
pub(crate) mod sys;
pub(crate) mod ui;
pub(crate) mod value;

#[cfg(test)]
pub(crate) mod test_support;

mod module;
pub(crate) use module::{install_module, module_table};

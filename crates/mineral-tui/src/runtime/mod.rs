//! 运行时层:应用状态与渲染装饰类型、播放镜像,以及与后端进程的连接、退出信号与数据预热。

pub mod action;
pub mod cover_colors;
pub mod cover_encode;
pub mod cover_fetch;
pub mod daemon;
pub mod filter;
pub mod keymap;
pub mod playback;
pub mod prefetch;
pub(crate) mod reload;
pub mod remote;
pub mod signal;
pub mod state;
pub mod ui_prefs;
pub mod view_model;

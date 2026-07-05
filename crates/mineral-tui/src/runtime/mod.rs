//! 运行时层:应用状态与渲染装饰类型、播放镜像,以及与后端进程的连接、退出信号与数据预热。

pub mod action;
pub mod cover;
pub mod daemon;
pub mod deep_search;
pub mod filter;
pub mod keymap;
pub(crate) mod line_input;
pub mod playback;
pub mod prefetch;
pub(crate) mod reload;
pub mod remote;
pub(crate) mod scroll;
pub mod signal;
pub mod state;
pub mod track_pos;
pub mod ui;
pub mod view_model;
pub(crate) mod window_title;

//! 运行时层:应用状态与渲染装饰类型、播放镜像,以及与后端进程的连接、退出信号与数据预热。

// PR-B(app.rs 接 dispatch)接通后移除这两个 allow(临时静默未接线的纯增类型)。
#[allow(dead_code)]
pub mod action;
pub mod cover_colors;
pub mod cover_encode;
pub mod cover_fetch;
pub mod daemon;
pub mod filter;
#[allow(dead_code)]
pub mod keymap;
pub mod playback;
pub mod prefetch;
pub mod remote;
pub mod signal;
pub mod state;
pub mod view_model;

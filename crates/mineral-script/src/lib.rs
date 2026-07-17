//! daemon 内嵌 Lua 脚本运行时。
//!
//! `mlua::Lua` 是 `Send + !Sync`,VM 归一条专用 OS 线程独占;daemon 经
//! channel 投递事件、脚本经 channel 发回命令,两侧消息都是结构化 Rust
//! 类型,Lua 值只活在 VM 边界。
//!
//! 接线顺序:[`ScriptHost::new`] → [`install_api`] → eval 用户脚本 →
//! [`ScriptRuntime::spawn`] 移交 VM。eval 失败由调用方弃整 VM(脚本是
//! 旁路增强,不拖垮 daemon 启动)。

mod api;
mod dispatch;
mod hooks;
mod host;
mod intercept;
mod message;
mod proc;
mod runtime;
mod sender;
mod watchdog;

pub use mlua;

pub use hooks::{
    BeforeDownloadCtx, BeforeStreamCtx, HookDecision, HookKind, HookMode, RewriteSpec,
};
pub use host::{ScriptHost, install_api, seed_web_url_templates};
pub use message::{
    ActionOutcome, ConfigOverrideOp, CurateOutcome, CuratedEntry, PlaylistBrief, PropKey,
    PropValue, QueryId, ResolveValue, ScriptCmd, ScriptEvent, TrackFinishedReason,
};
pub use proc::{SpawnId, SpawnResult, SpawnSpec, run_child};
pub use runtime::ScriptRuntime;
pub use sender::ScriptSender;
pub use watchdog::WatchdogConfig;

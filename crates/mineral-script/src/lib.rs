//! daemon 内嵌 Lua 脚本运行时。
//!
//! 事件 VM 由一条专用 OS 线程持有;daemon 经
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

/// 脚本线程或子进程执行失败。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 脚本专用线程无法启动。
    #[error("spawn mineral-script thread")]
    Thread(#[source] std::io::Error),

    /// 子进程启动或运行失败。
    #[error("{operation} `{program}` 失败")]
    Child {
        /// 子进程操作。
        operation: &'static str,

        /// 可执行文件。
        program: String,

        /// 原始进程错误。
        #[source]
        source: std::io::Error,
    },

    /// 复制模板或队列变换所需的脚本线程不可用。
    #[error("脚本未启用或线程已退出")]
    Unavailable,

    /// 无法读取脚本函数表或执行函数。
    #[error("{operation} 失败")]
    Lua {
        /// 操作名称。
        operation: &'static str,

        /// Lua 原始错误。
        #[source]
        source: mlua::Error,
    },

    /// 对应下标没有可调用的模板或变换函数。
    #[error("{kind} #{index} 没有可调用的函数")]
    MissingFunction {
        /// 函数用途。
        kind: &'static str,

        /// 配置中的零基下标。
        index: usize,

        /// Lua 原始错误。
        #[source]
        source: mlua::Error,
    },

    /// 队列变换返回的歌没有可解析的 id。
    #[error("返回的第 {index} 项 id 无法解析")]
    InvalidSongId {
        /// Lua 数组中的一起始下标。
        index: usize,

        /// 解析失败原因。
        #[source]
        source: mlua::Error,
    },

    /// 队列变换返回的歌表缺失或类型错误。
    #[error("返回的第 {index} 项{field}无效")]
    InvalidSongEntry {
        /// Lua 数组中的一起始下标。
        index: usize,

        /// 错误字段。
        field: &'static str,

        /// Lua 原始错误。
        #[source]
        source: mlua::Error,
    },
}

/// 脚本线程与子进程执行结果。
pub type Result<T> = std::result::Result<T, Error>;

pub use mlua;

pub use hooks::{
    BeforeDownloadCtx, BeforeStreamCtx, HookDecision, HookKind, HookMode, RewriteSpec,
};
pub use host::{ScriptHost, SourceWebUrls, install_api, seed_web_url_templates};
pub use message::{
    ActionOutcome, ConfigOverrideOp, CurateOutcome, CuratedEntry, PlaylistBrief, PropKey,
    PropValue, QueryId, ResolveValue, ScriptCmd, ScriptEvent, TrackFinishedReason,
};
pub use proc::{SpawnId, SpawnResult, SpawnSpec, run_child};
pub use runtime::ScriptRuntime;
pub use sender::ScriptSender;
pub use watchdog::WatchdogConfig;

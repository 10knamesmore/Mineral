//! daemon 与 TUI 的独立 Lua 执行宿主。
//!
//! daemon.lua 与 tui.lua 分别返回独立配置，setup 参数提供各自宿主 API。
//! [`ScriptRuntime`] 在线程启动前执行 daemon.lua 的 setup，再串行执行音乐查询与 hook。
//! [`evaluate_tui`] 保护 TUI 提供的配置求值操作并执行同 VM 的 setup;
//! [`TuiRuntime`] 保留复制模板闭包,不创建脚本线程。配置类型由宿主持有。

mod api;
mod copy;
mod dispatch;
mod hooks;
mod host;
mod intercept;
mod lua_stub;
mod message;
mod projection;
pub mod registry;
mod runtime;
mod sender;
mod setup;
mod tui;
mod watchdog;

/// 脚本求值或 daemon 回调执行失败。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 脚本专用线程无法启动。
    #[error("spawn mineral-script thread")]
    Thread(#[source] std::io::Error),

    /// 队列变换所需的 daemon 脚本线程不可用。
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

    /// 对应下标没有可调用的本地复制模板。
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

    /// 指定操作名称没有可调用的队列变换。
    #[error("队列变换 {name} 没有可调用的函数")]
    MissingQueueTransform {
        /// daemon 配置中的唯一操作名。
        name: String,

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

/// 脚本求值与回调执行结果。
pub type Result<T> = std::result::Result<T, Error>;

pub use mlua;

pub use copy::CopyTemplateCtx;
pub use hooks::{
    BeforeDownloadCtx, BeforeStreamCtx, HookDecision, HookKind, HookMode, RewriteSpec,
};
pub use host::{ScriptHost, SourceWebUrls, install_daemon_api, seed_web_url_templates};
pub use lua_stub::TYPES_META;
pub use message::{
    ConfigOverrideOp, CurateOutcome, CuratedEntry, PlaylistBrief, QueryId, ResolveValue, ScriptCmd,
};
pub use runtime::ScriptRuntime;
pub use sender::ScriptSender;
pub use tui::{TuiCommand, TuiLoad, TuiRuntime, evaluate_tui};
pub use watchdog::WatchdogConfig;

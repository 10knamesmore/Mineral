//! Mineral 用户配置(Lua):强类型 [`Config`] 的单一真相源。
//!
//! 内置 `default.lua` 经 `include_str!` 编入二进制,启动期与用户 `config.lua` 深合并后
//! 整表落成 [`Config`]。声明面(主题 / 键位 / 音频 / 缓存等)经 getter 读取;事件 hooks
//! 等可编程层的 host API 在此只有 no-op stub([`inject_noop_host`]),活实现由 daemon
//! 脚本运行时注入。键字符串与语义键的统一表示见 [`keys`]。

pub mod keys;

mod check;
mod init;
mod loader;
mod lua_stub;
mod schema;

/// 配置资产写入或内置默认配置加载失败。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 初始化配置目录或写入配置文件失败。
    #[error("{operation} {} 失败", .path.display())]
    InitIo {
        /// 失败的操作。
        operation: &'static str,

        /// 目标路径。
        path: std::path::PathBuf,

        /// 原始 IO 错误。
        #[source]
        source: std::io::Error,
    },

    /// Lua 虚拟机操作失败。
    #[error("{operation} 失败")]
    Lua {
        /// 虚拟机操作。
        operation: &'static str,

        /// 原始 Lua 错误。
        #[source]
        source: mlua::Error,
    },

    /// 内置默认表无法落成配置,属于程序包错误。
    #[error("default.lua 无法落成 Config")]
    DefaultConfig {
        /// 配置字段和分类。
        #[source]
        warning: ConfigWarning,
    },
}

/// 配置初始化与加载结果。
pub type Result<T> = std::result::Result<T, Error>;

/// Lua 操作在配置管线中的通用转换;显式边界另行标出操作。
impl From<mlua::Error> for Error {
    /// 保留底层 Lua 错误供调用方沿 source 链诊断。
    fn from(source: mlua::Error) -> Self {
        Self::Lua {
            operation: "处理 Lua 配置",
            source,
        }
    }
}

pub use check::render_check;
pub use init::{InitOutcome, run_init};
pub use loader::{
    ConfigWarning, DaemonLoad, default_tree, from_tree, inject_noop_host, load, load_with_vm,
    merge_tree, nest_path,
};
pub use schema::*;

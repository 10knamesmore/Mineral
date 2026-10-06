//! daemon 与 TUI 共用的 Lua 配置机械能力。
//!
//! 宿主提供默认源码、schema 与回调摘取函数;本 crate 只负责求值、
//! 深合并、数据落型、字段诊断和资产写出,不持有业务配置或宿主目录布局。

pub mod de;
mod init;
mod loader;

/// 配置资产写入、Lua 求值或配置落型失败。
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
    #[error("内置默认配置无法落型")]
    DefaultConfig {
        /// 配置字段和分类。
        #[source]
        warning: ConfigWarning,
    },

    /// 源码求值后的配置无法序列化或落型;不返回部分成功产物。
    #[error("配置无效")]
    InvalidConfig {
        /// 配置转换或落型失败的诊断。
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

pub use init::{InitOutcome, create_dir, overwrite, recreate_dir, write_if_absent};
pub use loader::{
    ConfigWarning, FileLoad, defaults, deserialize_tree, extract_setup, from_source, load_file,
    merge_tree, nest_path, table_path,
};

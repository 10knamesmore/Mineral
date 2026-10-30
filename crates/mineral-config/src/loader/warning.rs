//! 指定宿主配置文件读取、求值或落型失败的诊断。

use std::io;
use std::path::PathBuf;

/// 用户配置的非致命问题;文件加载失败回落所属宿主的默认。
/// 内置默认损坏由 [`crate::Error`] 表达。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigWarning {
    /// 读取用户配置失败；文件不存在不生成告警。
    #[error("failed to read configuration {}; using defaults", .path.display())]
    Read {
        /// 读取失败的配置文件路径。
        path: PathBuf,

        /// 文件系统返回的原始错误。
        #[source]
        source: io::Error,
    },

    /// 用户配置求值失败(语法错 / 运行期错 / 返回非表)。
    #[error("failed to evaluate configuration {}; using defaults", .path.display())]
    Eval {
        /// 求值失败的配置文件路径。
        path: PathBuf,

        /// Lua 返回的原始错误,包含可定位的脚本信息。
        #[source]
        source: mlua::Error,
    },

    /// Lua 表无法转成配置树。
    #[error("failed to serialize configuration; using defaults")]
    Serialize {
        /// Lua 值序列化失败的原始错误。
        #[source]
        source: serde_json::Error,
    },

    /// 合并后的配置树无法落型。`path` 是出错字段路径,如 `audio.volume`。
    #[error("invalid configuration{location}; using defaults", location = warning_location(.path.as_deref()))]
    Deserialize {
        /// 出错字段路径；顶层错误或无法定位字段时为 `None`。
        path: Option<String>,

        /// JSON 配置树落型失败的原始错误。
        #[source]
        source: serde_json::Error,
    },
}

/// 路径缺席时诊断配置整体,否则诊断具体字段。
fn warning_location(path: Option<&str>) -> String {
    match path {
        Some(path) => format!(" field `{path}`"),
        None => " type".to_owned(),
    }
}

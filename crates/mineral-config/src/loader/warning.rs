//! 加载过程中的非致命问题类型。出现即表示该层(或整份)用户配置回落了默认。

/// 加载过程中的非致命问题。仅针对**用户** `config.lua`;内置 `default.lua` 损坏
/// 是程序员错误,由守卫测试拦截、启动期 fail,不进本类型。
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ConfigWarning {
    /// 用户 `config.lua` eval 失败(语法错 / 运行期错 / 返回非表)。`detail` 含 file:line。
    Eval {
        /// 人类可读详情(Lua 错误首行 + 定位)。
        detail: String,
    },

    /// 合并后整表反序列化失败。`path` 是出错字段路径,如 `audio.volume`。
    Deserialize {
        /// 出错字段路径(可能为空,表示顶层 / 无法定位)。
        path: String,

        /// 类型 / 取值错误详情。
        detail: String,
    },
}

impl std::fmt::Display for ConfigWarning {
    /// 单行展示(供 toast):eval 错带定位,反序列化错带字段路径。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Eval { detail } => write!(f, "config.lua 加载失败,已回落默认:{detail}"),
            Self::Deserialize { path, detail } if path.is_empty() => {
                write!(f, "config.lua 类型错误,已回落默认:{detail}")
            }
            Self::Deserialize { path, detail } => {
                write!(f, "config.lua 字段 `{path}` 错误,已回落默认:{detail}")
            }
        }
    }
}

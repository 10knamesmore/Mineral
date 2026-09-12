//! 加载并校验完整的阻塞调用清单，配置错误直接报告给编译器。

use rustc_session::Session;
use serde::Deserialize;

/// 本次编译使用的阻塞调用配置；显式配置完整替换内置清单。
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Config {
    /// 需要禁止的直接调用；空清单表示不禁止任何调用。
    pub(super) calls: Vec<BlockingCall>,
}

/// 一个阻塞调用的匹配条件与诊断说明。
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BlockingCall {
    /// 包含 crate 名的定义路径，按完整字符串精确匹配。
    pub(super) path: String,

    /// 诊断 note 中展示的阻塞原因与替代方向。
    pub(super) reason: String,
}

impl Config {
    /// 读取显式配置，缺少配置节时读取内置清单；无效配置发出编译错误。
    ///
    /// # Params:
    ///   - `sess`: 接收配置错误诊断的编译会话。
    ///
    /// # Return:
    ///   校验成功的完整配置；解析失败或条目为空白时返回 `None`。
    pub(crate) fn load(sess: &Session) -> Option<Self> {
        let config = match dylint_linting::config::<Self>("mineral_blocking_in_async") {
            Ok(Some(config)) => config,
            Ok(None) => match toml::from_str::<Self>(include_str!("default.toml")) {
                Ok(config) => config,
                Err(error) => {
                    let error = error.to_string();
                    sess.dcx().err(format!(
                        "invalid default configuration for mineral_blocking_in_async: {}",
                        error.trim()
                    ));
                    return None;
                }
            },
            Err(error) => {
                let error = error.to_string();
                sess.dcx().err(format!(
                    "invalid configuration for mineral_blocking_in_async: {}",
                    error.trim()
                ));
                return None;
            }
        };

        if config
            .calls
            .iter()
            .any(|call| call.path.trim().is_empty() || call.reason.trim().is_empty())
        {
            sess.dcx()
                .err("mineral_blocking_in_async: each call requires a nonempty path and reason");
            return None;
        }

        Some(config)
    }
}

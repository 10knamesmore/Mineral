//! netease 内部的结构化错误载体。

use thiserror::Error;

/// 网易云业务层非 200 code 的结构化错误,经 `color_eyre::Report` 携带,
/// 在 channel 边界(`channel::map_err`)downcast 还原成
/// `mineral_channel_core::Error` 的对应变体。
///
/// 途中可以随意 `.wrap_err(..)` 加上下文(downcast 沿 source 链查),
/// 但**不要把它格式化成字符串后重新 `eyre!`**——链一断,映射就退化成
/// `Error::Other`,TUI 只能给通用失败提示。
#[derive(Debug, Clone, Error)]
#[error("api code {code}: {message}")]
pub struct ApiCodeError {
    /// 网易云返回的业务 code(301 未登录 / 512 风控或容量 / 502 已存在等)。
    pub code: i64,

    /// 服务端 `message` / `msg` 字段(无则空串)。
    pub message: String,
}

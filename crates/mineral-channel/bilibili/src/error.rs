//! bilibili 内部的结构化错误载体。

use thiserror::Error;

/// B站业务层非 0 code 的结构化错误,经 `color_eyre::Report` 携带,在 channel 边界
/// (`channel::map_err`)downcast 还原成 `mineral_channel_core::Error` 的对应变体。
///
/// B站信封 `{code, message, data}` 的 `code != 0` 即业务失败(`0` = 成功,与网易的 `200` 不同)。
/// 途中可 `.wrap_err(..)` 加上下文(downcast 沿 source 链查),但**别**格式化成字符串后重新
/// `eyre!`——链一断,映射退化成 `Error::Other`。
#[derive(Debug, Clone, Error)]
#[error("api code {code}: {message}")]
pub struct ApiCodeError {
    /// B站返回的业务 code(如 `-101` 未登录 / `-352` 风控或签名失效 / `-404` 无此视频)。
    pub code: i64,

    /// 服务端 `message` 字段(无则空串)。
    pub message: String,
}

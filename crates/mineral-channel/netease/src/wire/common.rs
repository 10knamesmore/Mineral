//! 跨端点共享的 envelope / 包裹类型。

use serde::Deserialize;

/// 网易云 API 顶层 envelope（`code` + 业务字段平铺）。
#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    /// 业务状态码（200 = 成功）。
    pub code: i64,

    /// 错误描述（成功时通常缺失）。
    #[serde(default)]
    pub message: Option<String>,

    /// 实际业务数据，扁平化在 envelope 里。
    #[serde(flatten)]
    pub data: T,
}

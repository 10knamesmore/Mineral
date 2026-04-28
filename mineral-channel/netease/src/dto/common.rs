use serde::Deserialize;

/// 网易云 API 顶层 envelope。
#[derive(Debug, Deserialize)]
pub struct Envelope<T> {
    pub code: i64,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(flatten)]
    pub data: T,
}

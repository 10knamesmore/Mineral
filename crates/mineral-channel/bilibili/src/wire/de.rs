//! 带字段路径的 JSON 反序列化。
//!
//! 裸 `serde_json::from_value` 失败只给 `invalid type: null, expected a string`,
//! 不含字段路径(从 `Value` 反序列化连 line / col 都是 0),线上排查无从下手——
//! 不知道是第几条、哪个字段。这里用 `serde_path_to_error` 包一层,把出错位置
//! 的字段路径(如 `result[3].bvid`)并进错误信息。

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{Error, Result};

/// 从 [`Value`] 反序列化为 `T`,失败时错误信息带字段路径。
///
/// # Params:
///   - `value`: 待反序列化的 JSON。
///
/// # Return:
///   成功返回 `T`；失败保留字段路径及 serde 原始原因。
pub(crate) fn from_value<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_path_to_error::deserialize(value).map_err(|e| {
        let context = e.path().to_string();
        Error::Parse {
            context,
            source: Box::new(e.into_inner()),
        }
    })
}

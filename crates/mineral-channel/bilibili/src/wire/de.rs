//! 带字段路径的 JSON 反序列化。
//!
//! 裸 `serde_json::from_value` 失败只给 `invalid type: null, expected a string`,
//! 不含字段路径(从 `Value` 反序列化连 line / col 都是 0),线上排查无从下手——
//! 不知道是第几条、哪个字段。这里用 `serde_path_to_error` 包一层,把出错位置
//! 的字段路径(如 `result[3].bvid`)并进错误信息。

use color_eyre::eyre::eyre;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

/// 从 [`Value`] 反序列化为 `T`,失败时错误信息带字段路径。
///
/// # Params:
///   - `value`: 待反序列化的 JSON。
///
/// # Return:
///   成功得 `T`;失败得形如 ``at `result[3].bvid`: invalid type: null, expected a string`` 的错误。
pub(crate) fn from_value<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_path_to_error::deserialize(value).map_err(|e| eyre!("at `{}`: {}", e.path(), e.inner()))
}

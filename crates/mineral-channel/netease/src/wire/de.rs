//! 带字段路径的 JSON 反序列化。
//!
//! 裸 `serde_json::from_value` 失败只给 `invalid type: null, expected a string`,
//! 不含字段路径(从 `Value` 反序列化连 line / col 都是 0),线上排查无从下手——
//! 不知道是第几条、哪个字段。这里用 `serde_path_to_error` 包一层,把出错位置
//! 的字段路径(如 `[12].al.name`)并进错误信息。

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
///   成功得 `T`;失败得形如 ``at `[12].al.name`: invalid type: null, expected a string`` 的错误。
pub(crate) fn from_value<T: DeserializeOwned>(value: Value) -> Result<T> {
    serde_path_to_error::deserialize(value).map_err(|e| eyre!("at `{}`: {}", e.path(), e.inner()))
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;

    use super::from_value;
    use crate::wire::song::AlbumSong;

    #[test]
    fn error_carries_field_path() -> super::Result<()> {
        // 第 2 条的 id 类型错误(给字符串):错误应精确到 `[1].id`,而非裸 "invalid type"。
        // (用 id 而非 al.name —— 后者已被 string_or_null 容忍,不再报错。)
        let raw = serde_json::json!([
            { "id": 1, "name": "ok", "ar": [], "al": { "id": 2, "name": null }, "dt": 0 },
            { "id": "not-a-number", "name": "x", "ar": [], "al": { "id": 3, "name": "a" }, "dt": 0 }
        ]);
        let Err(err) = from_value::<Vec<AlbumSong>>(raw) else {
            return Err(eyre!("expected deserialize to fail on bad id type"));
        };
        let msg = format!("{err}");
        assert!(
            msg.contains("[1].id"),
            "want field path in error, got: {msg}"
        );
        Ok(())
    }
}

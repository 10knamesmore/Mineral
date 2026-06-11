//! 带字段路径的 JSON 反序列化。
//!
//! 裸 `serde_json::from_value` 失败只给 `invalid type: null, expected a string`,
//! 不含字段路径(从 `Value` 反序列化连 line / col 都是 0),线上排查无从下手——
//! 不知道是第几条、哪个字段。这里用 `serde_path_to_error` 包一层,把出错位置
//! 的字段路径(如 `[12].al.name`)并进错误信息。

use color_eyre::eyre::eyre;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

/// 本模块内部统一的 result 别名,屏蔽 color-eyre 全名。
type Result<T> = color_eyre::Result<T>;

/// 把 `null` 收成空串。网易云对失效 / 下架歌曲会把 name 类字段(歌名 / 艺术家名 /
/// 专辑名)返回 `null`,裸 `String` 反序列化会炸掉整批(已实锤:歌单 5036089714 的
/// `[2].al.name` 为 null);`#[serde(default)]` 只兜底字段缺失、兜不住显式 `null`,
/// 故这里把 `null` 与缺失统一收成空串。
pub(crate) fn string_or_null<'de, D>(de: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(de)?.unwrap_or_default())
}

/// 反序列化 `Vec<T>`,跳过其中的 `null` 元素。网易云对失效 / 下架歌曲会在 `ar`
/// (艺术家)数组里塞 `null`(已实锤:歌单 5036089714 的「张洲」`ar` 为 `[null]`),
/// 裸 `Vec<Artist>` 会炸(`null` 不是 struct)。这里把 `null` 元素直接丢弃。
pub(crate) fn vec_skip_null<'de, D, T>(de: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Vec::<Option<T>>::deserialize(de)?
        .into_iter()
        .flatten()
        .collect())
}

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

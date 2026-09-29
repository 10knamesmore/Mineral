//! 带字段路径的 JSON 反序列化。
//!
//! 裸 `serde_json::from_value` 失败只给 `invalid type: null, expected a string`,
//! 不含字段路径(从 `Value` 反序列化连 line / col 都是 0),线上排查无从下手——
//! 不知道是第几条、哪个字段。这里用 `serde_path_to_error` 包一层,把出错位置
//! 的字段路径(如 `[12].al.name`)并进错误信息。

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::{Error, Result};

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

/// 反序列化 `Vec<T>`,同时容忍**字段整体为 `null`** 与**元素为 `null`**。
///
/// 网易云 `/weapi/user/playlist` 的歌单列表项把 `tracks` / `trackIds` 返回显式 `null`
/// (不是键缺失、也不是空数组):`#[serde(default)]` 只兜键缺失、[`vec_skip_null`] 又要求
/// 整体是序列(遇顶层 `null` 报 `invalid type: null, expected a sequence`)。这里先把顶层
/// `null` 收成空 `Vec`,再丢弃数组里的 `null` 元素——两层 null 都不炸。
pub(crate) fn null_or_vec_skip_null<'de, D, T>(de: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(Option::<Vec<Option<T>>>::deserialize(de)?
        .unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::from_value;
    use crate::wire::song::AlbumSong;

    #[test]
    fn error_carries_field_path() -> super::Result<()> {
        // 第 2 条的 id 类型错误(给字符串):错误应精确到 `[1].id`,而非裸 "invalid type"。
        // al.name 使用 string_or_null,因此以严格的 id 字段触发错误。
        let raw = serde_json::json!([
            { "id": 1, "name": "ok", "ar": [], "al": { "id": 2, "name": null }, "dt": 0 },
            { "id": "not-a-number", "name": "x", "ar": [], "al": { "id": 3, "name": "a" }, "dt": 0 }
        ]);
        let Err(err) = from_value::<Vec<AlbumSong>>(raw) else {
            return Err(crate::Error::InvalidData {
                field: "expected invalid id type",
            });
        };
        assert!(matches!(
            err,
            crate::Error::Parse { context, source }
                if context == "[1].id" && source.is::<serde_json::Error>()
        ));
        Ok(())
    }
}

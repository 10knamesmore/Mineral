//! 各段共享的字段级反序列化 helper。
//!
//! 字节容量字段用 `u64`,但 Lua 的 `^` 幂运算总产 float(`10 * 1024 ^ 3`),
//! 故反序列化层容忍数值为整数或非负有限浮点(floor 后转 `u64`),非法值报错
//! 经路径冒泡。

/// 把数值反序列化为 `u64`,容忍非负有限浮点(Lua `^` 幂运算产物):
/// 整数直接取;浮点 floor 后转;负/非有限/越界报错(经 `serde_path_to_error` 带路径)。
///
/// # Params:
///   - `deserializer`: 字段反序列化器
///
/// # Return:
///   非负整数容量字节
pub(crate) fn u64_lossy<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let value = serde_json::Value::deserialize(deserializer)?;
    let serde_json::Value::Number(n) = &value else {
        return Err(serde::de::Error::custom(format!(
            "期望数值容量(字节),得到 `{value}`"
        )));
    };
    if let Some(u) = n.as_u64() {
        return Ok(u);
    }
    if let Some(f) = n.as_f64()
        && f.is_finite()
        && f >= 0.0
        && let Some(u) = num_traits::cast::cast::<f64, u64>(f.floor())
    {
        return Ok(u);
    }
    Err(serde::de::Error::custom(format!(
        "容量须为非负且不越界的整数字节,得到 `{value}`"
    )))
}

/// 反序列化字符串列表,容忍 Lua 空表 `{}`。
///
/// Lua 的 `{}` 既是空数组也是空表,mlua 落成空 map;而 `Vec<String>` 期望 sequence。
/// 非空数组(如 `{"mock"}`,有整数键 1..n)正常走 seq。此 helper 把空 map 视作空
/// 列表、逐元素取字符串,其余报错(经 `serde_path_to_error` 带路径)。
///
/// # Params:
///   - `deserializer`: 字段反序列化器
///
/// # Return:
///   字符串列表(可能为空)
pub(crate) fn string_list<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    let value = serde_json::Value::deserialize(deserializer)?;
    match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(|item| match item {
                serde_json::Value::String(s) => Ok(s),
                other => Err(serde::de::Error::custom(format!(
                    "列表元素须为字符串,得到 `{other}`"
                ))),
            })
            .collect(),
        serde_json::Value::Object(map) if map.is_empty() => Ok(Vec::new()),
        other => Err(serde::de::Error::custom(format!(
            "期望字符串数组(空表亦可),得到 `{other}`"
        ))),
    }
}

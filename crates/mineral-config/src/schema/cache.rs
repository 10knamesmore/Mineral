//! 缓存容量段(字节)。
//!
//! 容量字段用 `u64`,但 Lua 的 `^` 幂运算总产 float(`10 * 1024 ^ 3`),故反序列化
//! 层容忍数值为整数或非负有限浮点(floor 后转 `u64`),非法值报错经路径冒泡。

use mineral_config_macros::config_section;
use serde::Deserialize;

/// 缓存容量段。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct CacheConfig {
    /// 音频本体缓存容量上限(字节)。
    #[serde(deserialize_with = "de_u64_lossy")]
    audio_capacity: u64,

    /// 封面磁盘缓存容量上限(字节)。
    #[serde(deserialize_with = "de_u64_lossy")]
    cover_capacity: u64,
}

/// 把数值反序列化为 `u64`,容忍非负有限浮点(Lua `^` 幂运算产物):
/// 整数直接取;浮点 floor 后转;负/非有限/越界报错(经 `serde_path_to_error` 带路径)。
///
/// # Params:
///   - `deserializer`: 字段反序列化器
///
/// # Return:
///   非负整数容量字节
fn de_u64_lossy<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
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

#[cfg(test)]
mod tests {
    use super::CacheConfig;

    #[test]
    fn accepts_integer_and_float_bytes() -> color_eyre::Result<()> {
        // 整数(Lua integer 路径)。
        let c: CacheConfig = serde_json::from_value(
            serde_json::json!({ "audio_capacity": 1024_u64, "cover_capacity": 512_u64 }),
        )?;
        assert_eq!(*c.audio_capacity(), 1024);
        // 浮点(Lua `10 * 1024 ^ 3` 路径)。
        let c: CacheConfig = serde_json::from_value(
            serde_json::json!({ "audio_capacity": 10737418240.0_f64, "cover_capacity": 1073741824.0_f64 }),
        )?;
        assert_eq!(*c.audio_capacity(), 10 * 1024 * 1024 * 1024);
        assert_eq!(*c.cover_capacity(), 1024 * 1024 * 1024);
        Ok(())
    }

    #[test]
    fn rejects_negative() {
        assert!(
            serde_json::from_value::<CacheConfig>(
                serde_json::json!({ "audio_capacity": -1.0_f64, "cover_capacity": 1.0_f64 })
            )
            .is_err(),
            "负容量应报错"
        );
    }
}

//! 缓存容量段(字节):daemon 侧磁盘缓存的配额。
//!
//! client 进程的封面缓存预算(磁盘 + RAM)不在此段,见 `tui.cover.cache`。

use mineral_config_macros::config_section;

use crate::schema::de;

/// 缓存容量段。
#[config_section]
pub struct CacheConfig {
    /// 音频本体缓存容量上限(字节);可写算式如 `10 * 1024 ^ 3`。
    #[serde(deserialize_with = "de::u64_lossy")]
    audio_capacity: u64,
}

#[cfg(test)]
mod tests {
    use super::CacheConfig;

    #[test]
    fn accepts_integer_and_float_bytes() -> color_eyre::Result<()> {
        // 整数(Lua integer 路径)。
        let c: CacheConfig = serde_json::from_value(serde_json::json!({
            "audio_capacity": 1024_u64,
        }))?;
        assert_eq!(*c.audio_capacity(), 1024);
        // 浮点(Lua `10 * 1024 ^ 3` 路径)。
        let c: CacheConfig = serde_json::from_value(serde_json::json!({
            "audio_capacity": 10737418240.0_f64,
        }))?;
        assert_eq!(*c.audio_capacity(), 10 * 1024 * 1024 * 1024);
        Ok(())
    }

    #[test]
    fn rejects_negative() {
        assert!(
            serde_json::from_value::<CacheConfig>(serde_json::json!({
                "audio_capacity": -1.0_f64,
            }))
            .is_err(),
            "负容量应报错"
        );
    }
}

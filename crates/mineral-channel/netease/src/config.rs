//! `NeteaseChannel` 的构造参数([`NeteaseConfig`])。

/// `NeteaseChannel` 的构造参数。私有字段 + builder 构造 + getter 读取。
///
/// **所有字段必填,本类型不携带默认值**:默认值的唯一真相源是 mineral-config 的
/// `default.lua`(`sources.netease` 段),由消费侧(`mineral` 启动链 / CLI)映射传入,
/// 避免两处默认漂移。
#[non_exhaustive]
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct NeteaseConfig {
    /// 最大并发连接数(`0` 表示不限)。
    max_connections: usize,

    /// 代理地址(`None` = 不走代理),例如 `socks5://127.0.0.1:1080`。
    proxy: Option<String>,

    /// 单次请求超时(秒)。
    timeout_secs: u64,
}

#[cfg(test)]
mod tests {
    use super::NeteaseConfig;

    /// builder 逐旋钮生效(布线验证,不打网络)。
    #[test]
    fn builder_sets_fields() {
        let c = NeteaseConfig::builder()
            .timeout_secs(7)
            .max_connections(3)
            .proxy(Some("socks5://127.0.0.1:1080".to_owned()))
            .build();
        assert_eq!(*c.timeout_secs(), 7);
        assert_eq!(*c.max_connections(), 3);
        assert_eq!(c.proxy().as_deref(), Some("socks5://127.0.0.1:1080"));
    }
}

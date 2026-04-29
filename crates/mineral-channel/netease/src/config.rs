/// `NeteaseChannel` 的构造参数。
#[derive(Clone, Debug, Default)]
pub struct NeteaseConfig {
    /// 最大并发连接数(`0` 表示不限)。
    pub max_connections: usize,
    /// 代理地址,例如 `socks5://127.0.0.1:1080`。
    pub proxy: Option<String>,
}

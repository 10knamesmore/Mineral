//! `NeteaseChannel` 的构造参数([`NeteaseConfig`])。

use std::num::{NonZeroU32, NonZeroUsize};
use std::time::Duration;

/// `NeteaseChannel` 的构造参数
#[non_exhaustive]
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct NeteaseConfig {
    /// 最大并发连接数(`0` 表示不限)。
    max_connections: usize,

    /// 代理地址(`None` = 不走代理),例如 `socks5://127.0.0.1:1080`。
    proxy: Option<String>,

    /// 单次请求超时(秒)。
    timeout_secs: u64,

    /// 歌单浏览和完整加载共用的请求批次参数。
    playlist_fetch: PlaylistFetchConfig,

    /// 专辑详情发送速率与读取端点的限流退避。
    requests: RequestsConfig,

    /// 专辑详情的持久缓存期限。
    album_cache: AlbumCacheConfig,
}

/// 每次成功抓取后确定缓存期限；零或负的计算结果直接视为已过期，读取不续期。
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct AlbumCacheConfig {
    /// 缓存基础期限，天；允许零期限。
    ttl_days: u32,

    /// 基础期限两侧的随机增减上限，天；零表示偏移量只可能为零。
    ttl_jitter_days: u32,
}

/// 由 daemon 来源配置注入的请求控制参数。
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct RequestsConfig {
    /// 所有专辑详情调用共享的每秒请求数；初次请求与重试均计入，突发容量固定为一。
    album_detail_requests_per_second: NonZeroU32,

    /// 每项对应一次额外尝试前的等待时间；每次调用独立推进该序列。
    retry_delays: Vec<Duration>,
}

/// 网易歌曲详情的批次大小与同一歌单内的并发上限
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct PlaylistFetchConfig {
    /// 浏览歌单时每批覆盖的 ID 数量，也是单次歌曲详情请求的上限；必须大于零。
    batch_size: NonZeroUsize,

    /// 一个歌单最多同时发出的歌曲详情请求数；必须大于零。
    max_concurrent: NonZeroUsize,
}

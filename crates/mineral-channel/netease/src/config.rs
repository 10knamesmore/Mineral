//! `NeteaseChannel` 的构造参数([`NeteaseConfig`])。

use std::num::NonZeroUsize;

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

    /// 歌单浏览和完整加载共用的请求批次参数。
    playlist_fetch: PlaylistFetchConfig,
}

/// 网易歌曲详情的批次大小与同一歌单内的并发上限；默认值由 default.lua 提供。
#[derive(Clone, Debug, typed_builder::TypedBuilder, derive_getters::Getters)]
pub struct PlaylistFetchConfig {
    /// 浏览歌单时每批覆盖的 ID 数量，也是单次歌曲详情请求的上限；必须大于零。
    batch_size: NonZeroUsize,

    /// 一个歌单最多同时发出的歌曲详情请求数；必须大于零。
    max_concurrent: NonZeroUsize,
}

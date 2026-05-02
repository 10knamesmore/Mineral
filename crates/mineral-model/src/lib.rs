//! 跨 channel 的统一数据模型。
//!
//! 所有 channel 的输出最终是平铺合并的——除 `source` 标记外没有 channel-specific
//! 字段,上层 UI 拿到 `Vec<Song>` 不需要知道哪首来自哪。

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )
)]

/// 专辑及其曲目列表。
pub mod album;
/// 艺人及其代表曲目。
pub mod artist;
/// 跨 channel 的归一化音质等级。
pub mod bitrate;
/// 各类资源(歌、专辑、艺人、歌单、用户)的 ID newtype。
pub mod ids;
/// 一首歌的歌词集合(LRC、YRC、翻译、罗马音)。
pub mod lyrics;
/// 一首歌的可播放 URL + 元信息。
pub mod play_url;
/// 歌单及其曲目列表。
pub mod playlist;
/// 在 Song / Album 中嵌入的 ArtistRef / AlbumRef 轻引用。
pub mod refs;
/// 搜索的目标类型枚举。
pub mod search;
/// 歌曲核心结构。
pub mod song;
/// 标识资源来源 channel 的枚举(Netease / Local 等)。
pub mod source;
/// 区分远端 / 本地的媒体资源 URL。
pub mod url;

pub use album::Album;
pub use artist::Artist;
pub use bitrate::BitRate;
pub use ids::{AlbumId, ArtistId, PlaylistId, SongId, UserId};
pub use lyrics::Lyrics;
pub use play_url::PlayUrl;
pub use playlist::Playlist;
pub use refs::{AlbumRef, ArtistRef};
pub use search::SearchKind;
pub use song::Song;
pub use source::SourceKind;
pub use url::MediaUrl;

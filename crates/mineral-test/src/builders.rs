//! [`Song`] 构造器:一个最小默认 + 一组函数式装饰,组合出测试要的形态。
//!
//! 设计成函数式装饰(`with_source(song("x"), SourceKind::Local)`)而非多参构造,
//! 避免 `song("x", None, 0, ...)` 这种谜语调用。

use mineral_model::{ArtistId, ArtistRef, Song, SongId, SourceKind};

/// 造一首最小 `Song`:来源 `Netease`、`name == id`、时长 0、无艺人/专辑/封面。
///
/// # Params:
///   - `id`: 歌曲 ID(同时用作歌名)
///
/// # Return:
///   填好默认值的 `Song`,再用 `with_*` 装饰。
pub fn song(id: &str) -> Song {
    Song {
        source: SourceKind::Netease,
        id: SongId::from(id),
        name: id.to_owned(),
        artists: Vec::new(),
        album: None,
        duration_ms: 0,
        cover_url: None,
        source_url: None,
    }
}

/// 给一首 `Song` 挂上单个艺人(`id == name`)。
///
/// # Params:
///   - `s`: 原 `Song`
///   - `artist`: 艺人名
///
/// # Return:
///   `artists` 被替换为该艺人的 `Song`。
pub fn with_artist(mut s: Song, artist: &str) -> Song {
    s.artists = vec![ArtistRef {
        id: ArtistId::from(artist),
        name: artist.to_owned(),
    }];
    s
}

/// 改一首 `Song` 的歌名。
///
/// # Params:
///   - `s`: 原 `Song`
///   - `name`: 新歌名
///
/// # Return:
///   `name` 被替换的 `Song`。
pub fn with_name(mut s: Song, name: &str) -> Song {
    s.name = name.to_owned();
    s
}

/// 改一首 `Song` 的来源 channel。
///
/// # Params:
///   - `s`: 原 `Song`
///   - `source`: 新来源
///
/// # Return:
///   `source` 被替换的 `Song`。
pub fn with_source(mut s: Song, source: SourceKind) -> Song {
    s.source = source;
    s
}

/// 设一首 `Song` 的时长(ms)。
///
/// # Params:
///   - `s`: 原 `Song`
///   - `duration_ms`: 时长(ms)
///
/// # Return:
///   `duration_ms` 被替换的 `Song`。
pub fn with_duration(mut s: Song, duration_ms: u64) -> Song {
    s.duration_ms = duration_ms;
    s
}

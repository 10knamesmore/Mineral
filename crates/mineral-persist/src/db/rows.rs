//! db 行结构体与 mineral-model 互转。

use sqlx::FromRow;

/// `song_meta` 行。
#[derive(FromRow)]
pub(crate) struct SongMetaRow {
    /// 来源稳定名。
    pub namespace: String,

    /// 源内裸值。
    pub song_value: String,

    /// 歌名。
    pub name: String,

    /// 专辑裸 id(可空)。
    pub album_id: Option<String>,

    /// 专辑名(可空)。
    pub album_name: Option<String>,

    /// 时长毫秒(sqlite INTEGER → i64)。
    pub duration_ms: i64,

    /// 封面 MediaUrl 序列化串(可空)。
    pub cover_url: Option<String>,
}

/// `song_artists` 行(仅取重建艺人所需字段)。
#[derive(FromRow)]
pub(crate) struct SongArtistRow {
    /// 艺人裸 id。
    pub artist_id: String,

    /// 艺名。
    pub artist_name: String,
}

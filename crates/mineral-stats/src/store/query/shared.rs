//! 跨查询子模块共享的 id 重建 helper + row 类型。

use mineral_model::{SongId, SourceKind};
use sea_orm::sea_query;

use crate::vocab::FinishReason;

/// 由裸 ns + song_value 重建 `SongId`。
pub(super) fn song_id(ns: &str, value: &str) -> SongId {
    SongId::new(SourceKind::from_name(ns), value)
}

/// 播放流水行(history tail);`overview::recent_plays` 与 `discoveries::edge_play` 共用
/// ——`song` 由 ns+value 重建故经中转。
#[derive(sea_orm::FromQueryResult)]
pub(super) struct PlayTailRow {
    /// 来源 name。
    pub(super) ns: String,

    /// 裸歌曲 id。
    pub(super) song_value: String,

    /// 起播时刻 epoch ms。
    pub(super) started_at: i64,

    /// 实际收听 ms。
    pub(super) listen_ms: i64,

    /// 结束原因(TEXT → 枚举)。
    pub(super) finish_reason: FinishReason,
}

/// 聚合结果中的具名列。
#[derive(Clone, Copy, Debug, sea_orm::DeriveColumn)]
pub(super) enum ReportColumn {
    /// 播放次数。
    Plays,
    /// 收听毫秒数。
    ListenMs,
    /// 专辑名称。
    AlbumName,
    /// 艺人名称。
    ArtistName,
    /// 上下文类型。
    Kind,
    /// 上下文身份。
    Reference,
    /// 展示名称。
    Name,
    /// 会话数量。
    Sessions,
    /// 最早播放时间。
    FirstPlayAt,
    /// 最晚播放时间。
    LastPlayAt,
    /// 完整播放次数。
    Completed,
    /// 跳过次数。
    Skipped,
    /// 单曲跳过次数。
    Skips,
    /// 不同歌曲数量。
    DistinctSongs,
    /// 活跃日期数。
    ActiveDays,
    /// 单曲最近播放时间。
    LastPlayedAt,
}

/// 选择时间窗口内的播放事实。
pub(super) fn plays_in(
    range: std::ops::Range<i64>,
) -> sea_orm::Select<crate::entity::plays::Entity> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    crate::entity::plays::Entity::find()
        .filter(crate::entity::plays::Column::StartedAt.gte(range.start))
        .filter(crate::entity::plays::Column::StartedAt.lt(range.end))
}

/// 播放事实与歌曲维表按来源和歌曲身份关联。
pub(super) fn song_dimension() -> sea_orm::RelationDef {
    use crate::entity::{plays, songs};
    use sea_orm::EntityTrait;
    plays::Entity::belongs_to(songs::Entity)
        .from((plays::Column::Ns, plays::Column::SongValue))
        .to((songs::Column::Ns, songs::Column::SongValue))
        .into()
}

/// 播放事实与有序艺人维表按来源和歌曲身份关联。
pub(super) fn artist_dimension() -> sea_orm::RelationDef {
    use crate::entity::{plays, song_artists};
    use sea_orm::EntityTrait;
    plays::Entity::belongs_to(song_artists::Entity)
        .from((plays::Column::Ns, plays::Column::SongValue))
        .to((song_artists::Column::Ns, song_artists::Column::SongValue))
        .into()
}

/// SQLite 的日期投影函数。
#[derive(sea_orm::sea_query::Iden)]
pub(super) struct Date;

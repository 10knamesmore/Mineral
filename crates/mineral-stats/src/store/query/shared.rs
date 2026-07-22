//! 跨查询子模块共享的 id 重建 helper + row 类型。

use mineral_model::{SongId, SourceKind};

use crate::vocab::FinishReason;

/// 由裸 ns + song_value 重建 `SongId`。
pub(super) fn song_id(ns: &str, value: &str) -> SongId {
    SongId::new(SourceKind::from_name(ns), value)
}

/// 播放流水行(history tail);`overview::recent_plays` 与 `discoveries::edge_play` 共用
/// ——`song` 由 ns+value 重建故经中转。
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

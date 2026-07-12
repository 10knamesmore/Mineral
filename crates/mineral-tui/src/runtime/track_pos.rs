//! 歌单内光标位置记忆:退出曲目列表时按歌单记一条 [`TrackPos`] 双锚,下次进入
//! 恢复;`behavior.remember_track_pos = "persist"` 档经 `tui.db` 的 `track_pos`
//! 专表落盘(结构化行,库层禁 JSON blob)。
//!
//! 双锚语义:优先按 `song_id` 在当前曲目里定位(歌单增删后仍指向同一首),
//! 该歌已被删则退回 `index` 并钳到末行——纯 index 会在增删后指错歌,纯 id 会在
//! 歌被删后彻底失锚,两者互补。

use mineral_model::{PlaylistId, SongId};
use rustc_hash::FxHashMap;

use crate::runtime::view_model::SongView;

/// 歌单 id → 记忆位置 的内存表。
pub type TrackPosMap = FxHashMap<PlaylistId, TrackPos>;

/// 一个歌单的记忆位置(双锚 + 屏上相对行)。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackPos {
    /// 记录时光标所在歌曲(优先锚)。
    pub song_id: SongId,

    /// 记录时的行下标(兜底锚:歌被删后钳到末行)。
    pub index: usize,

    /// 记录时光标在视口内的相对行(`sel - 视口首行`)。恢复时反推视口首行,
    /// 让该行回到离开时的屏上位置;终端尺寸变了由渲染端 clamp 兜底(瞬时,无平移)。
    pub screen_row: usize,
}

impl TrackPos {
    /// 在给定曲目列表中解析回行下标:优先按 `song_id` 定位,找不到时退回
    /// `index` 钳到末行;空列表恒为 0。
    ///
    /// # Params:
    ///   - `tracks`: 当前歌单的曲目列表
    ///
    /// # Return:
    ///   恢复后的光标行下标。
    pub fn resolve(&self, tracks: &[SongView]) -> usize {
        tracks
            .iter()
            .position(|sv| sv.data.id == self.song_id)
            .unwrap_or_else(|| self.index.min(tracks.len().saturating_sub(1)))
    }
}

/// 进歌单时曲目未就绪而挂起的恢复:曲目到达且光标未被用户动过时补落位。
pub struct PendingRestore {
    /// 目标歌单(曲目到达时与事件的歌单 id 比对)。
    pub playlist: PlaylistId,

    /// 待恢复的位置。
    pub pos: TrackPos,
}

/// 内存表 → `track_pos` 表行(落盘用)。
///
/// # Params:
///   - `map`: 内存表
///
/// # Return:
///   结构化行(顺序不定;主键 = 歌单 id);下标溢出 i64(理论不可达)返回 `Err`。
pub fn to_rows(map: &TrackPosMap) -> color_eyre::Result<Vec<mineral_persist::TrackPosRow>> {
    map.iter()
        .map(|(playlist, pos)| {
            Ok(mineral_persist::TrackPosRow {
                playlist: playlist.clone(),
                song: pos.song_id.clone(),
                index: u64::try_from(pos.index)?,
                screen_row: u64::try_from(pos.screen_row)?,
            })
        })
        .collect()
}

/// `track_pos` 表行 → 内存表(启动读回用)。
///
/// # Params:
///   - `rows`: 库中全部行
///
/// # Return:
///   内存表;下标超出 usize(库损坏)返回 `Err`(调用方降级空表)。
pub fn from_rows(rows: Vec<mineral_persist::TrackPosRow>) -> color_eyre::Result<TrackPosMap> {
    rows.into_iter()
        .map(|row| {
            Ok((
                row.playlist,
                TrackPos {
                    song_id: row.song,
                    index: usize::try_from(row.index)?,
                    screen_row: usize::try_from(row.screen_row)?,
                },
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mineral_model::{PlaylistId, SourceKind};

    use crate::runtime::view_model::SongView;
    use crate::test_support::song;

    use super::{TrackPos, TrackPosMap, from_rows, to_rows};

    /// 把若干歌名包成 SongView 列表(loved / plays 取默认)。
    fn views(names: &[&str]) -> Vec<SongView> {
        names
            .iter()
            .map(|n| SongView {
                data: song(n),
                loved: false,
                plays: None,
            })
            .collect()
    }

    /// 双锚解析:song_id 仍在列表时按 id 定位,无视 index 漂移。
    #[test]
    fn resolve_prefers_song_id() {
        let tracks = views(&["甲", "乙", "丙"]);
        let pos = TrackPos {
            song_id: song("丙").id,
            index: 0, // index 已过时(歌单头部插了歌),id 锚应胜出
            screen_row: 0,
        };
        assert_eq!(pos.resolve(&tracks), 2);
    }

    /// 双锚解析:歌被删后退回 index;index 越界钳到末行;空列表恒 0。
    #[test]
    fn resolve_falls_back_to_clamped_index() {
        let tracks = views(&["甲", "乙"]);
        let gone = TrackPos {
            song_id: song("已删除的歌").id,
            index: 1,
            screen_row: 0,
        };
        assert_eq!(gone.resolve(&tracks), 1, "id 失锚退回 index");

        let overflow = TrackPos {
            song_id: song("已删除的歌").id,
            index: 99,
            screen_row: 0,
        };
        assert_eq!(overflow.resolve(&tracks), 1, "index 越界钳到末行");
        assert_eq!(overflow.resolve(&[]), 0, "空列表恒 0");
    }

    /// 行投影 round-trip:to_rows 后 from_rows 还原同一张表(含跨 source 的歌单 key)。
    #[test]
    fn rows_round_trip() -> color_eyre::Result<()> {
        let mut map = TrackPosMap::default();
        map.insert(
            PlaylistId::new(SourceKind::NETEASE, "p1"),
            TrackPos {
                song_id: song("甲").id,
                index: 3,
                screen_row: 7,
            },
        );
        map.insert(
            PlaylistId::new(SourceKind::SHELF, "p1"),
            TrackPos {
                song_id: song("乙").id,
                index: 0,
                screen_row: 0,
            },
        );
        let back = from_rows(to_rows(&map)?)?;
        assert_eq!(back, map);
        Ok(())
    }
}

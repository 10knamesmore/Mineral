//! 歌单内光标位置记忆:退出曲目列表时按歌单记一条 [`TrackPos`] 双锚,下次进入
//! 恢复;`behavior.remember_track_pos = "persist"` 档经 ui_prefs 以 JSON 落盘。
//!
//! 双锚语义:优先按 `song_id` 在当前曲目里定位(歌单增删后仍指向同一首),
//! 该歌已被删则退回 `index` 并钳到末行——纯 index 会在增删后指错歌,纯 id 会在
//! 歌被删后彻底失锚,两者互补。

use mineral_model::{PlaylistId, SongId};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

use crate::runtime::view_model::SongView;

/// 歌单 id → 记忆位置 的内存表。
pub type TrackPosMap = FxHashMap<PlaylistId, TrackPos>;

/// 一个歌单的记忆位置(双锚 + 屏上相对行)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackPos {
    /// 记录时光标所在歌曲(优先锚)。
    pub song_id: SongId,

    /// 记录时的行下标(兜底锚:歌被删后钳到末行)。
    pub index: usize,

    /// 记录时光标在视口内的相对行(`sel - 视口首行`)。恢复时反推视口首行,
    /// 让该行回到离开时的屏上位置;终端尺寸变了由渲染端 clamp 兜底(瞬时,无平移)。
    /// 缺省(旧落盘数据)= 0,即恢复到视口顶行。
    #[serde(default)]
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

/// 落盘 wire 条目。map 序列化成条目数组而不是 JSON object——ID 是结构化类型,
/// 不强求扁平字符串 key,结构由 serde 自描述。
#[derive(Serialize, Deserialize)]
struct WireEntry {
    /// 歌单 id。
    playlist: PlaylistId,

    /// 该歌单的记忆位置。
    pos: TrackPos,
}

/// 把内存表编码成落盘 JSON。
///
/// # Params:
///   - `map`: 内存表
///
/// # Return:
///   JSON 字符串;序列化失败返回 `Err`(调用方 warn 后放弃本次落盘)。
pub fn encode(map: &TrackPosMap) -> color_eyre::Result<String> {
    let entries: Vec<WireEntry> = map
        .iter()
        .map(|(playlist, pos)| WireEntry {
            playlist: playlist.clone(),
            pos: pos.clone(),
        })
        .collect();
    Ok(serde_json::to_string(&entries)?)
}

/// 从落盘 JSON 解码回内存表。
///
/// # Params:
///   - `raw`: 落库的 JSON 字符串
///
/// # Return:
///   内存表;脏 JSON 返回 `Err`(调用方降级空表)。
pub fn decode(raw: &str) -> color_eyre::Result<TrackPosMap> {
    let entries: Vec<WireEntry> = serde_json::from_str(raw)?;
    Ok(entries.into_iter().map(|e| (e.playlist, e.pos)).collect())
}

#[cfg(test)]
mod tests {
    use mineral_model::{PlaylistId, SourceKind};

    use crate::runtime::view_model::SongView;
    use crate::test_support::song;

    use super::{TrackPos, TrackPosMap, decode, encode};

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

    /// wire round-trip:encode 后 decode 还原同一张表(含跨 source 的歌单 key)。
    #[test]
    fn wire_round_trips() -> color_eyre::Result<()> {
        let mut map = TrackPosMap::default();
        map.insert(
            PlaylistId::new(SourceKind::NETEASE, "p1"),
            TrackPos {
                song_id: song("甲").id,
                index: 3,
                screen_row: 0,
            },
        );
        map.insert(
            PlaylistId::new(SourceKind::LOCAL, "p1"),
            TrackPos {
                song_id: song("乙").id,
                index: 0,
                screen_row: 0,
            },
        );
        let raw = encode(&map)?;
        let back = decode(&raw)?;
        assert_eq!(back, map);
        Ok(())
    }

    /// 脏 JSON decode 返回 Err(调用方降级空表),不 panic。
    #[test]
    fn decode_rejects_garbage() {
        assert!(decode("not json").is_err());
        assert!(decode(r#"{"wrong": "shape"}"#).is_err());
    }

    /// 旧落盘数据兼容:缺 `screen_row` 字段的条目解码成功,缺省 0(视口顶行)。
    #[test]
    fn decode_defaults_missing_screen_row() -> color_eyre::Result<()> {
        let mut map = TrackPosMap::default();
        let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
        map.insert(
            pid.clone(),
            TrackPos {
                song_id: song("甲").id,
                index: 3,
                screen_row: 7,
            },
        );
        let mut v: serde_json::Value = serde_json::from_str(&encode(&map)?)?;
        let stripped = v
            .get_mut(0)
            .and_then(|e| e.get_mut("pos"))
            .and_then(serde_json::Value::as_object_mut)
            .and_then(|pos| pos.remove("screen_row"));
        assert!(stripped.is_some(), "前置:wire 里确有 screen_row 字段");
        let back = decode(&serde_json::to_string(&v)?)?;
        assert_eq!(
            back.get(&pid).map(|p| p.screen_row),
            Some(0),
            "缺字段缺省 0"
        );
        Ok(())
    }
}

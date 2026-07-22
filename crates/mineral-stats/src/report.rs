//! 查询结果领域类型 + 查询期口径。
//!
//! 数值聚合全在 stats.db(见 [`crate::StatsStore`] 的查询方法);歌名 / 艺人 / 专辑名
//! 出自库内 `songs` / `song_artists` 维表,不跨库回查。
//!
//! `top_albums` / `top_artists` 是口味口径:按 `plays` JOIN `songs`(专辑)/
//! `song_artists`(艺人)聚合——「听了哪张专辑 / 哪个艺人的歌」。与之对应的 context
//! 口径(「从某专辑 / 艺人详情页起播」)走 [`crate::StatsStore::top_contexts`],
//! 返回 [`ContextSlice`]。

use mineral_model::{AlbumId, ArtistId, SongId};
use serde::Serialize;
use typed_builder::TypedBuilder;

use crate::vocab::FinishReason;

/// 查询期口径(不进落库,改动可回溯重算全部历史)。
#[derive(Clone, Copy, Debug, TypedBuilder)]
#[non_exhaustive]
pub struct ReportOptions {
    /// 有效播放阈值 ms:`listen_ms` 不足此值的行不计入榜 / 比率(流水照记)。
    min_listen_ms: i64,

    /// 各 top 榜长度上限。
    top_limit: i64,
}

impl ReportOptions {
    /// 有效播放阈值 ms。
    pub fn min_listen_ms(&self) -> i64 {
        self.min_listen_ms
    }

    /// top 榜长度上限。
    pub fn top_limit(&self) -> i64 {
        self.top_limit
    }
}

/// 榜单排序口径。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopBy {
    /// 按播放次数。
    Plays,

    /// 按收听时长。
    Time,
}

/// 时段分桶维度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BucketBy {
    /// 一天中的小时(0-23)。
    Hour,

    /// 星期(0=周日 .. 6=周六,sqlite strftime('%w'))。
    Weekday,

    /// 月份(1-12)。
    Month,
}

/// 总量汇总。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub struct Totals {
    /// 收听 ms 总和。
    pub listen_ms: i64,

    /// 播放次数。
    pub plays: i64,

    /// 完播数(finish_reason=eof)。
    pub completed: i64,

    /// 跳歌数(finish_reason=skip)。
    pub skipped: i64,

    /// 涉及的不同歌曲数。
    pub distinct_songs: i64,

    /// 活跃天数(有播放的不同本地日期数)。
    pub active_days: i64,
}

/// top 歌曲一项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TopSong {
    /// 歌曲 id。
    pub song: SongId,

    /// songs 维表回查的歌名;未覆盖为 `None`(展示层回落 id)。
    pub name: Option<String>,

    /// 播放次数。
    pub plays: i64,

    /// 收听 ms 总和。
    pub listen_ms: i64,
}

/// top 专辑一项(按专辑语境 `context_ref` 聚合)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TopAlbum {
    /// 专辑 id(由 `songs.ns` + `album_id` 重建)。
    pub album: AlbumId,

    /// 组内任意非空的显示名快照;全组缺名为 `None`(展示层回落 id)。
    pub name: Option<String>,

    /// 该专辑内歌曲的播放次数(口味口径:「听了这张专辑的歌」)。
    pub plays: i64,

    /// 收听 ms 总和。
    pub listen_ms: i64,
}

/// top 艺人一项(按艺人语境 `context_ref` 聚合)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TopArtist {
    /// 艺人 id(从 `plays.context_ref` 的 qualified 串重建)。
    pub artist: ArtistId,

    /// 组内任意非空的显示名快照;全组缺名为 `None`(展示层回落 id)。
    pub name: Option<String>,

    /// 从该艺人起播的次数。
    pub plays: i64,

    /// 收听 ms 总和。
    pub listen_ms: i64,
}

/// 时段分桶一项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Bucket {
    /// 桶键(Hour 0-23 / Weekday 0-6 / Month 1-12)。
    pub key: i64,

    /// 该桶播放次数。
    pub plays: i64,

    /// 该桶收听 ms。
    pub listen_ms: i64,
}

/// 一个「值 → 计数」的分布项。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Slice {
    /// 分类值(如来源 name / 格式串 / 音质档);列可空时用空串占位由查询决定。
    pub value: String,

    /// 该值的播放次数。
    pub plays: i64,
}

/// 各维度分布。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub struct Distributions {
    /// 按来源 ns。
    pub by_source: Vec<Slice>,

    /// 按发起方式 origin_kind。
    pub by_origin: Vec<Slice>,

    /// 按播放模式 play_mode。
    pub by_play_mode: Vec<Slice>,

    /// 按音频格式 audio_format(NULL 归入空串桶)。
    pub by_format: Vec<Slice>,

    /// 按音质档 quality(NULL 归入空串桶)。
    pub by_quality: Vec<Slice>,

    /// 按音频本体来源位置 playback_origin。
    pub by_playback_origin: Vec<Slice>,

    /// 无损播放次数(is_lossless=1)。
    pub lossless_plays: i64,
}

/// 一个队列语境的播放聚合(top contexts:最常从哪听)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ContextSlice {
    /// 语境类型(search / playlist / album / artist / manual / unknown)。
    pub kind: String,

    /// 语境引用(搜索词 / qualified id);无为 `None`(manual / unknown)。
    pub reference: Option<String>,

    /// 组内任意非空的显示名快照;search / manual / unknown 或全组缺名为 `None`。
    pub name: Option<String>,

    /// 该语境的播放次数。
    pub plays: i64,

    /// 该语境的收听 ms 总和。
    pub listen_ms: i64,
}

/// 一张事件表的行数(event_summary:各交互事件量一览)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EventCount {
    /// 表名(= 事件 kind 名)。
    pub table: String,

    /// 行数。
    pub count: i64,
}

/// 一个「标签 → 计数」分桶项(event_summary 的各维分桶通用:outcome / decision / event /
/// 搜索词 / 动作名 / fetch_kind 等)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Tally {
    /// 分桶标签。
    pub label: String,

    /// 该桶行数。
    pub count: i64,
}

/// full 档事件盘点(event_summary:各交互事件的量与分桶)。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub struct EventSummary {
    /// 各事件表行数(窗口内)。
    pub table_counts: Vec<EventCount>,

    /// top 搜索词(按 query_hash 去重;标签取原文,缺则散列)。
    pub top_searches: Vec<Tally>,

    /// love 新增按 origin 分桶(仅 loved=true:user / import)。
    pub love_by_origin: Vec<Tally>,

    /// 下载三态计数(downloaded / skipped / failed)。
    pub downloads_by_outcome: Vec<Tally>,

    /// 缓存收割计数(cached / discarded)。
    pub harvests_by_outcome: Vec<Tally>,

    /// top 下钻页(fetch_kind 计次)。
    pub top_fetches: Vec<Tally>,

    /// top 具名动作(action name 计次)。
    pub top_actions: Vec<Tally>,

    /// 补救漏斗:hook_fires 按 decision 分桶(continue / rewrite / skip)。
    pub hooks_by_decision: Vec<Tally>,

    /// 无缝率:gapless_boundaries 按 result 分桶(adopt / fallback)。
    pub gapless_by_result: Vec<Tally>,

    /// 脚本健康:script_lifecycle 按 event 分桶(reload_ok / reload_fail / …)。
    pub script_by_event: Vec<Tally>,
}

/// 会话续航聚合(endurance:一次坐下能听多久)。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub struct Endurance {
    /// 会话数。
    pub sessions: i64,

    /// 平均会话时长 ms。
    pub avg_ms: i64,

    /// 最长会话时长 ms。
    pub longest_ms: i64,

    /// 最长连续听歌天数 streak(UTC 日;窗口内有播放的连续日的最长游程)。
    pub streak_days: i64,
}

/// 最近播放流水的一行(CLI `stats history` tail)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PlayTail {
    /// 歌曲 id(名字由 server 回查;CLI 展示回落 qualified id)。
    pub song: SongId,

    /// 起播时刻 epoch ms。
    pub started_at: i64,

    /// 实际收听 ms。
    pub listen_ms: i64,

    /// 结束原因。
    pub finish_reason: FinishReason,
}

/// 发现盘点(discoveries:窗口内首播新歌清单 + 首 / 末播放行)。
#[derive(Clone, Debug, PartialEq, Eq, Default, Serialize)]
pub struct Discoveries {
    /// 窗口内首播的新歌(按首播时刻升序,至多 limit 首)。
    pub new_songs: Vec<SongId>,

    /// 窗口内最早的一次播放行;无播放为 `None`。
    pub first_play: Option<PlayTail>,

    /// 窗口内最晚的一次播放行;无播放为 `None`。
    pub last_play: Option<PlayTail>,
}

/// 埋点系统自身状态(CLI `stats status`:时间覆盖 + 各区行数)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StatusReport {
    /// plays 行数。
    pub plays: i64,

    /// sessions 行数。
    pub sessions: i64,

    /// 全部事件表行数之和。
    pub events: i64,

    /// 最早播放起点 epoch ms;无播放为 `None`。
    pub first_play_at: Option<i64>,

    /// 最晚播放起点 epoch ms;无播放为 `None`。
    pub last_play_at: Option<i64>,
}

/// 单曲汇总(QuerySongStats 改口用,全量窗口)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SongSummary {
    /// 播放次数。
    pub plays: i64,

    /// 跳歌次数。
    pub skips: i64,

    /// 收听 ms 总和。
    pub listen_ms: i64,

    /// 最后播放时刻;从未播放为 `None`。
    pub last_played_at: Option<i64>,
}

/// 一条带展示名的榜项(top songs / albums / artists 装配后通用)。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct NamedEntry {
    /// qualified id(`namespace:value`;回查失败时展示层回落它)。
    pub id: String,

    /// 库内直出的展示名;缺失为 `None`。
    pub name: Option<String>,

    /// 播放次数。
    pub plays: i64,

    /// 收听 ms 总和。
    pub listen_ms: i64,
}

/// stats.db 直出的原始盘点(名字已随查询 JOIN 就位),[`combine`] 的输入。
#[derive(Clone, Debug, Default)]
pub struct RawReport {
    /// 总量。
    pub totals: Totals,

    /// top 歌曲。
    pub top_songs: Vec<TopSong>,

    /// top 专辑(context 聚合)。
    pub top_albums: Vec<TopAlbum>,

    /// top 艺人(context 聚合)。
    pub top_artists: Vec<TopArtist>,

    /// 各维分布。
    pub distributions: Distributions,

    /// 小时分桶。
    pub hourly: Vec<Bucket>,

    /// 发现盘点。
    pub discoveries: Discoveries,

    /// 续航。
    pub endurance: Endurance,

    /// 事件盘点。
    pub events: EventSummary,
}

/// 一份装配好的完整盘点报告(§8.1 全套,名字随库直出)。
#[derive(Clone, Debug, Serialize)]
#[non_exhaustive]
pub struct StatsReport {
    /// 总量。
    pub totals: Totals,

    /// top 歌曲(带名)。
    pub top_songs: Vec<NamedEntry>,

    /// top 专辑(带名)。
    pub top_albums: Vec<NamedEntry>,

    /// top 艺人(带名)。
    pub top_artists: Vec<NamedEntry>,

    /// 各维分布。
    pub distributions: Distributions,

    /// 小时分桶。
    pub hourly: Vec<Bucket>,

    /// 发现盘点。
    pub discoveries: Discoveries,

    /// 续航。
    pub endurance: Endurance,

    /// 事件盘点。
    pub events: EventSummary,
}

/// 纯函数:把 stats.db 直出的原始聚合装配成完整报告。
///
/// 名字已由查询层就地 JOIN / 快照聚合得出(stats.db 自足,不回查其他数据库);缺名
/// 保持 `None`,展示层回落 id。无 IO——server 出报告与将来 TUI 盘点页复用同一装配。
///
/// # Params:
///   - `raw`: stats.db 直出的原始聚合
///
/// # Return:
///   装配好的报告
pub fn combine(raw: RawReport) -> StatsReport {
    let named = |id: String, name: Option<String>, plays: i64, listen_ms: i64| NamedEntry {
        id,
        name,
        plays,
        listen_ms,
    };
    StatsReport {
        totals: raw.totals,
        top_songs: raw
            .top_songs
            .into_iter()
            .map(|t| named(t.song.qualified(), t.name, t.plays, t.listen_ms))
            .collect(),
        top_albums: raw
            .top_albums
            .into_iter()
            .map(|t| named(t.album.qualified(), t.name, t.plays, t.listen_ms))
            .collect(),
        top_artists: raw
            .top_artists
            .into_iter()
            .map(|t| named(t.artist.qualified(), t.name, t.plays, t.listen_ms))
            .collect(),
        distributions: raw.distributions,
        hourly: raw.hourly,
        discoveries: raw.discoveries,
        endurance: raw.endurance,
        events: raw.events,
    }
}

#[cfg(test)]
mod tests {
    use super::{RawReport, TopSong, combine};
    use mineral_model::{SongId, SourceKind};

    /// combine:query 层直出的名原样进 `NamedEntry`,缺名落 `None`(id 恒在供回落展示)。
    #[test]
    fn combine_carries_query_names() -> color_eyre::Result<()> {
        let raw = RawReport {
            top_songs: vec![
                TopSong {
                    song: SongId::new(SourceKind::NETEASE, "1"),
                    name: Some("稻香".to_owned()),
                    plays: 5,
                    listen_ms: 100,
                },
                TopSong {
                    song: SongId::new(SourceKind::NETEASE, "2"),
                    name: None,
                    plays: 3,
                    listen_ms: 60,
                },
            ],
            ..Default::default()
        };
        let report = combine(raw);
        assert_eq!(report.top_songs.len(), 2);
        let first = report
            .top_songs
            .first()
            .ok_or_else(|| color_eyre::eyre::eyre!("无首项"))?;
        assert_eq!(first.id, "netease:1");
        assert_eq!(first.name.as_deref(), Some("稻香"), "维表名原样穿透");
        let second = report
            .top_songs
            .get(1)
            .ok_or_else(|| color_eyre::eyre::eyre!("无次项"))?;
        assert_eq!(second.name, None, "维表未覆盖落 None");
        assert_eq!(second.id, "netease:2", "id 恒在供回落展示");
        Ok(())
    }
}

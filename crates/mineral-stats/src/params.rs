//! 采集侧运行时参数与 gating 逻辑。
//!
//! server 把 `stats` 配置段折算成 [`StatsParams`] 交给 recorder;热路径调用方只做
//! 这里的 gating(档位放不放行、来源早丢、搜索词模式)再决定组不组装命令。默认值不
//! 在此(在 default.lua),故所有字段 builder 必填——空集合也是从 config 来的空。

use rustc_hash::{FxHashMap, FxHashSet};
use typed_builder::TypedBuilder;

/// 采集档位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    /// 零写入。
    Off,

    /// 播放 + 会话(core 本体)。
    Core,

    /// 全谱交互。
    Full,
}

/// 搜索词落库模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SearchQueryMode {
    /// 原文入库。
    Raw,

    /// 不可逆散列(保次数 / 去重,丢原文)。
    Hashed,

    /// 连搜索行都不记。
    Off,
}

/// 流水保留策略。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Retention {
    /// 永久(盘点跨年,默认)。
    Forever,

    /// 只保留最近 N 天。
    Days(u32),
}

/// 采集侧运行时参数(整体热更替换)。
#[derive(Clone, Debug, TypedBuilder)]
#[non_exhaustive]
pub struct StatsParams {
    /// 采集档位。
    level: Level,

    /// 逐 kind 覆盖(在 level 基线上微调);kind 名 → 是否采集。
    collect: FxHashMap<String, bool>,

    /// 搜索词模式。
    search_queries: SearchQueryMode,

    /// 采集期排除的来源 name 集合(不留痕,隐私语义)。
    exclude_sources: FxHashSet<String>,

    /// 会话 gap 阈值 ms(由 session_gap_minutes 折算)。
    gap_ms: i64,

    /// 保留策略。
    retention: Retention,
}

impl StatsParams {
    /// 是否记录播放 / 会话(core 本体,不受 collect 控制)。
    pub fn records_plays(&self) -> bool {
        self.level != Level::Off
    }

    /// 某全谱事件 kind 是否采集:level 基线(full=开 / core=关)+ collect 逐键覆盖。
    ///
    /// # Params:
    ///   - `kind`: 事件 kind 名(与 collect 配置键、事件表名同源)
    pub fn collects_event(&self, kind: &str) -> bool {
        if self.level == Level::Off {
            return false;
        }
        let baseline = self.level == Level::Full;
        self.collect.get(kind).copied().unwrap_or(baseline)
    }

    /// 某来源是否采集期排除(按 ns 早丢,不落任何行)。
    pub fn excludes_source(&self, ns: &str) -> bool {
        self.exclude_sources.contains(ns)
    }

    /// 采集档位。
    pub fn level(&self) -> Level {
        self.level
    }

    /// 搜索词模式。
    pub fn search_queries(&self) -> SearchQueryMode {
        self.search_queries
    }

    /// 会话 gap 阈值 ms。
    pub fn gap_ms(&self) -> i64 {
        self.gap_ms
    }

    /// 保留策略。
    pub fn retention(&self) -> Retention {
        self.retention
    }
}

#[cfg(test)]
mod tests {
    use super::{Level, Retention, SearchQueryMode, StatsParams};
    use rustc_hash::{FxHashMap, FxHashSet};

    /// 造一份指定档位、无覆盖、无排除的参数。
    fn params(level: Level) -> StatsParams {
        StatsParams::builder()
            .level(level)
            .collect(FxHashMap::default())
            .search_queries(SearchQueryMode::Raw)
            .exclude_sources(FxHashSet::default())
            .gap_ms(30 * 60 * 1000)
            .retention(Retention::Forever)
            .build()
    }

    #[test]
    fn off_collects_nothing() {
        let p = params(Level::Off);
        assert!(!p.records_plays());
        assert!(!p.collects_event("searches"));
    }

    #[test]
    fn core_records_plays_but_no_events_by_default() {
        let p = params(Level::Core);
        assert!(p.records_plays());
        assert!(!p.collects_event("searches"), "core 基线不采全谱事件");
    }

    #[test]
    fn full_records_events_by_default() {
        let p = params(Level::Full);
        assert!(p.records_plays());
        assert!(p.collects_event("searches"));
    }

    #[test]
    fn collect_override_turns_on_in_core() {
        let mut collect = FxHashMap::default();
        collect.insert("searches".to_owned(), true);
        let p = StatsParams::builder()
            .level(Level::Core)
            .collect(collect)
            .search_queries(SearchQueryMode::Raw)
            .exclude_sources(FxHashSet::default())
            .gap_ms(0)
            .retention(Retention::Forever)
            .build();
        assert!(p.collects_event("searches"), "core 下 collect 开个别");
        assert!(!p.collects_event("seeks"), "未覆盖的仍随 core 基线关");
    }

    #[test]
    fn collect_override_turns_off_in_full() {
        let mut collect = FxHashMap::default();
        collect.insert("seeks".to_owned(), false);
        let p = StatsParams::builder()
            .level(Level::Full)
            .collect(collect)
            .search_queries(SearchQueryMode::Raw)
            .exclude_sources(FxHashSet::default())
            .gap_ms(0)
            .retention(Retention::Forever)
            .build();
        assert!(!p.collects_event("seeks"), "full 下 collect 关个别");
        assert!(p.collects_event("searches"), "未覆盖的仍随 full 基线开");
    }

    #[test]
    fn exclude_sources_matches_by_ns() {
        let mut excl = FxHashSet::default();
        excl.insert("mock".to_owned());
        let p = StatsParams::builder()
            .level(Level::Full)
            .collect(FxHashMap::default())
            .search_queries(SearchQueryMode::Raw)
            .exclude_sources(excl)
            .gap_ms(0)
            .retention(Retention::Forever)
            .build();
        assert!(p.excludes_source("mock"));
        assert!(!p.excludes_source("netease"));
    }
}

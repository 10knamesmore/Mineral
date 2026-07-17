//! `stats` 配置段折算成 recorder 用的运行时参数。
//!
//! 只折采集侧旋钮(level / collect / search_queries / exclude_sources /
//! session_gap_minutes / retention_days);`report` 口径不进此处,报告装配时现读
//! effective 配置。config↔stats 是两个 crate 各自的枚举,这里做边缘互映。

use rustc_hash::FxHashSet;

use mineral_config::{
    RetentionDays as ConfigRetention, SearchQueryMode as ConfigSearchMode, StatsConfig,
    StatsLevel as ConfigLevel,
};
use mineral_stats::{Level, Retention, SearchQueryMode, StatsParams};

/// 把 `stats` 配置段折算成 [`StatsParams`](采集侧运行时参数)。
///
/// # Params:
///   - `cfg`: effective 配置的 `stats` 段
///
/// # Return:
///   recorder 用的运行时参数(整体热更替换)
pub fn params_from_config(cfg: &StatsConfig) -> StatsParams {
    let level = match cfg.level() {
        ConfigLevel::Off => Level::Off,
        ConfigLevel::Core => Level::Core,
        ConfigLevel::Full => Level::Full,
    };
    let search_queries = match cfg.search_queries() {
        ConfigSearchMode::Raw => SearchQueryMode::Raw,
        ConfigSearchMode::Hashed => SearchQueryMode::Hashed,
        ConfigSearchMode::Off => SearchQueryMode::Off,
    };
    let retention = match cfg.retention_days() {
        ConfigRetention::Forever => Retention::Forever,
        ConfigRetention::Days(days) => Retention::Days(*days),
    };
    // 分钟 → ms;配置值小,溢出兜 i64::MAX。
    let gap_ms = i64::try_from(*cfg.session_gap_minutes())
        .map(|m| m.saturating_mul(60_000))
        .unwrap_or(i64::MAX);
    let exclude_sources = cfg
        .exclude_sources()
        .iter()
        .cloned()
        .collect::<FxHashSet<String>>();
    // 未知 kind 名警告(core 本体的覆盖已在 config 落型期拒,到不了这):collect 里既非
    // 合法事件 kind 的键运行时静默不匹配,校验期出一条 warn 让用户察觉拼错 / 过时的键。
    for kind in cfg.collect().keys() {
        if !mineral_stats::is_event_kind(kind) {
            mineral_log::warn!(target: "stats", kind = kind.as_str(), "stats.collect 含未知事件 kind 名,该键无效");
        }
    }
    StatsParams::builder()
        .level(level)
        .collect(cfg.collect().clone())
        .search_queries(search_queries)
        .exclude_sources(exclude_sources)
        .gap_ms(gap_ms)
        .retention(retention)
        .build()
}

#[cfg(test)]
mod tests {
    use super::params_from_config;
    use mineral_stats::{Level, Retention, SearchQueryMode};

    #[test]
    fn folds_default_config() -> color_eyre::Result<()> {
        let config = mineral_config::Config::defaults()?;
        let params = params_from_config(config.stats());
        // 对齐 default.lua 的 stats 段。
        assert_eq!(params.level(), Level::Full);
        assert_eq!(params.search_queries(), SearchQueryMode::Raw);
        assert_eq!(params.retention(), Retention::Forever);
        assert_eq!(params.gap_ms(), 30 * 60_000);
        assert!(params.records_plays());
        assert!(params.collects_event("searches"), "full 档默认采集事件");
        assert!(!params.excludes_source("netease"));
        Ok(())
    }
}

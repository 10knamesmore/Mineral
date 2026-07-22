//! stats.db 聚合查询。
//!
//! 全部带时间窗 `range: Range<i64>`([start_ms, end_ms))、跨源。时段 / 日期分桶按 UTC
//! (deterministic;本地时区分桶是后续 refinement,可由 server 传 tz 偏移)。榜单 / 比率类
//! 接 [`crate::ReportOptions`] 的有效播放阈值——落库不过滤,口径在 SQL WHERE 生效。
//!
//! 按查询族拆分子模块([`overview`] 总量/流水、[`top`] 排行榜、[`distributions`] 分布、
//! [`discoveries`] 新发现、[`endurance`] 续航),`shared` 收口跨族共用的 id 重建 helper。

mod discoveries;
mod distributions;
mod endurance;
mod overview;
mod shared;
mod top;

#[cfg(test)]
mod test_support;

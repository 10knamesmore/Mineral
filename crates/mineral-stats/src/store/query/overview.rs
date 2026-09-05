//! 播放流水 / 埋点自身状态 / 总量 / 单曲汇总。

use std::ops::Range;

use crate::entity::{plays, sessions};
use color_eyre::eyre::WrapErr as _;
use mineral_model::SongId;
use sea_orm::sea_query::{Expr, ExprTrait, Func, Query};
use sea_orm::{
    ColumnTrait, EntityTrait, FromQueryResult, QueryFilter, QueryOrder, QuerySelect, QueryTrait,
};

use crate::report::{PlayTail, SongSummary, StatusReport, Totals};
use crate::store::StatsStore;
use crate::vocab::FinishReason;

use super::shared::{Date, PlayTailRow, ReportColumn, plays_in, song_id};

/// `status` 主查询单行(events 由各事件表另算,故不直落 StatusReport)。
#[derive(FromQueryResult)]
struct StatusRow {
    /// plays 行数。
    plays: i64,

    /// sessions 行数。
    sessions: i64,

    /// 最早播放起点;无播放为 NULL。
    first_play_at: Option<i64>,

    /// 最晚播放起点;无播放为 NULL。
    last_play_at: Option<i64>,
}

impl StatsStore {
    /// 最近播放流水(窗口内、可按来源过滤,按起播时刻倒序取前 `limit` 条)。
    ///
    /// # Params:
    ///   - `range`: 时间窗口 `[start_ms, end_ms)`(全量传 `0..i64::MAX`)
    ///   - `source`: 只看某来源 name(`None` = 全来源);`ns` 列直接比对裸 name 串
    ///   - `limit`: 取前几条
    ///
    /// # Return:
    ///   最近播放流水,最新在前
    pub async fn recent_plays(
        &self,
        range: Range<i64>,
        source: Option<&str>,
        limit: i64,
    ) -> color_eyre::Result<Vec<PlayTail>> {
        let Some(db) = self.pool() else {
            return Ok(Vec::new());
        };
        let mut query = plays_in(range).select_only().columns([
            plays::Column::Ns,
            plays::Column::SongValue,
            plays::Column::StartedAt,
            plays::Column::ListenMs,
            plays::Column::FinishReason,
        ]);
        if let Some(source) = source {
            query = query.filter(plays::Column::Ns.eq(source));
        }
        let rows = query
            .order_by_desc(plays::Column::StartedAt)
            .limit(u64::try_from(limit).ok())
            .into_model::<PlayTailRow>()
            .all(db)
            .await
            .wrap_err("recent_plays 查询失败")?;
        Ok(rows
            .into_iter()
            .map(|row| PlayTail {
                song: song_id(&row.ns, &row.song_value),
                started_at: row.started_at,
                listen_ms: row.listen_ms,
                finish_reason: row.finish_reason,
            })
            .collect())
    }

    /// 埋点系统自身状态:plays / sessions / 全部事件表行数 + 播放时间覆盖。
    pub async fn status(&self) -> color_eyre::Result<StatusReport> {
        let Some(db) = self.pool() else {
            return Ok(StatusReport {
                plays: 0,
                sessions: 0,
                events: 0,
                first_play_at: None,
                last_play_at: None,
            });
        };
        let query = Query::select()
            .expr_as(
                plays::Entity::find()
                    .select_only()
                    .expr(plays::Column::Id.count())
                    .into_query(),
                ReportColumn::Plays,
            )
            .expr_as(
                sessions::Entity::find()
                    .select_only()
                    .expr(sessions::Column::Id.count())
                    .into_query(),
                ReportColumn::Sessions,
            )
            .expr_as(
                plays::Entity::find()
                    .select_only()
                    .expr(plays::Column::StartedAt.min())
                    .into_query(),
                ReportColumn::FirstPlayAt,
            )
            .expr_as(
                plays::Entity::find()
                    .select_only()
                    .expr(plays::Column::StartedAt.max())
                    .into_query(),
                ReportColumn::LastPlayAt,
            )
            .to_owned();
        let row = StatusRow::find_by_statement(db.get_database_backend().build(&query))
            .one(db)
            .await
            .wrap_err("status 查询失败")?
            .ok_or_else(|| color_eyre::eyre::eyre!("status 聚合未返回行"))?;
        let mut events = 0_i64;
        for table in crate::store::prune::EVENT_TABLES {
            events += table
                .count(db, /*range*/ None)
                .await
                .wrap_err_with(|| format!("status count {} 失败", table.name()))?;
        }
        Ok(StatusReport {
            plays: row.plays,
            sessions: row.sessions,
            events,
            first_play_at: row.first_play_at,
            last_play_at: row.last_play_at,
        })
    }

    /// 总量:收听 ms / 播放次数 / 完播数 / 跳歌数 / 涉及歌曲数 / 活跃天数(UTC 日)。
    pub async fn totals(&self, range: Range<i64>) -> color_eyre::Result<Totals> {
        let Some(db) = self.pool() else {
            return Ok(Totals::default());
        };
        let distinct_songs = plays_in(range.clone())
            .select_only()
            .columns([plays::Column::Ns, plays::Column::SongValue])
            .distinct()
            .into_query();
        let song_count = Query::select()
            .expr(Expr::col(sea_orm::sea_query::Asterisk).count())
            .from_subquery(distinct_songs, ReportColumn::DistinctSongs)
            .to_owned();
        let day = Func::cust(Date).args([
            Expr::col((plays::Entity, plays::Column::StartedAt)).div(1000),
            Expr::value("unixepoch"),
        ]);
        plays_in(range)
            .select_only()
            .column_as(
                plays::Column::ListenMs.sum().if_null(0),
                ReportColumn::ListenMs,
            )
            .column_as(plays::Column::Id.count(), ReportColumn::Plays)
            .column_as(finish_count(FinishReason::Eof), ReportColumn::Completed)
            .column_as(finish_count(FinishReason::Skip), ReportColumn::Skipped)
            .column_as(Expr::from(song_count), ReportColumn::DistinctSongs)
            .column_as(Expr::from(day).count_distinct(), ReportColumn::ActiveDays)
            .into_model::<Totals>()
            .one(db)
            .await
            .wrap_err("totals 查询失败")?
            .ok_or_else(|| color_eyre::eyre::eyre!("totals 聚合未返回行"))
    }

    /// 返回 `QuerySongStats` 使用的单曲全量汇总；从未播放返回 `None`。
    pub async fn song_summary(&self, id: &SongId) -> color_eyre::Result<Option<SongSummary>> {
        let Some(db) = self.pool() else {
            return Ok(None);
        };
        let row = plays::Entity::find()
            .select_only()
            .column_as(plays::Column::Id.count(), ReportColumn::Plays)
            .column_as(finish_count(FinishReason::Eof), ReportColumn::Completed)
            .column_as(finish_count(FinishReason::Skip), ReportColumn::Skips)
            .column_as(
                plays::Column::ListenMs.sum().if_null(0),
                ReportColumn::ListenMs,
            )
            .column_as(plays::Column::StartedAt.max(), ReportColumn::LastPlayedAt)
            .filter(plays::Column::Ns.eq(id.namespace().name()))
            .filter(plays::Column::SongValue.eq(id.value()))
            .into_model::<SongSummary>()
            .one(db)
            .await
            .wrap_err("song_summary 查询失败")?
            .ok_or_else(|| color_eyre::eyre::eyre!("song_summary 聚合未返回行"))?;
        Ok((row.plays != 0).then_some(row))
    }
}

/// 统计指定结束原因；空集合的结果为零。
fn finish_count(reason: FinishReason) -> Expr {
    Expr::Case(Box::new(
        Expr::case(plays::Column::FinishReason.eq(reason), 1).finally(0),
    ))
    .sum()
    .if_null(0)
}

#[cfg(test)]
mod tests {
    use crate::report::TopBy;
    use crate::store::StatsStore;

    use super::super::shared::song_id;
    use super::super::test_support::{HOUR, T0, full_range, open_temp, options, seed};

    #[tokio::test]
    async fn totals_aggregates_all_fields() -> color_eyre::Result<()> {
        let (_d, store) = open_temp().await?;
        seed(&store).await?;
        let t = store.totals(full_range()).await?;
        assert_eq!(t.listen_ms, 60_000 + 70_000 + 5_000 + 120_000);
        assert_eq!(t.plays, 4);
        assert_eq!(t.completed, 3, "eof:A1 A2 C");
        assert_eq!(t.skipped, 1, "skip:B");
        assert_eq!(t.distinct_songs, 3);
        assert_eq!(t.active_days, 2, "day1 + day2");
        Ok(())
    }

    #[tokio::test]
    async fn song_summary_and_none_for_unknown() -> color_eyre::Result<()> {
        let (_d, store) = open_temp().await?;
        seed(&store).await?;
        let a = store
            .song_summary(&song_id("netease", "1"))
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("A 应有汇总"))?;
        assert_eq!(a.plays, 2);
        assert_eq!(a.completed, 2, "A 两次均自然播完");
        assert_eq!(a.skips, 0);
        assert_eq!(a.listen_ms, 130_000);
        assert_eq!(a.last_played_at, Some(T0 + 14 * HOUR + 60_000));

        let b = store
            .song_summary(&song_id("netease", "2"))
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("B 应有汇总"))?;
        assert_eq!(b.completed, 0, "B 只有 skip，不算完整播放");
        assert_eq!(b.skips, 1);

        assert!(
            store
                .song_summary(&song_id("netease", "999"))
                .await?
                .is_none()
        );
        Ok(())
    }

    /// 降级句柄:跨查询族(总量 / 排行 / 单曲汇总)均静默返回空结果,不报错。
    #[tokio::test]
    async fn disabled_queries_are_empty() -> color_eyre::Result<()> {
        let store = StatsStore::disabled();
        assert_eq!(store.totals(full_range()).await?.plays, 0);
        assert!(
            store
                .top_songs(full_range(), TopBy::Plays, &options(0))
                .await?
                .is_empty()
        );
        assert!(
            store
                .song_summary(&song_id("netease", "1"))
                .await?
                .is_none()
        );
        Ok(())
    }
}

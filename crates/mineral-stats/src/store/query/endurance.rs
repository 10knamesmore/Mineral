//! 会话续航:次数 / 平均 / 最长时长 + 最长连续听歌天数。

use std::ops::Range;

use super::shared::plays_in;
use crate::entity::{plays, sessions};
use color_eyre::eyre::WrapErr as _;
use sea_orm::sea_query::{self, Expr, ExprTrait, Func, Iden, Order, Query, WindowStatement};
use sea_orm::{ColumnTrait, ConnectionTrait, EntityTrait, QueryFilter, QuerySelect, QueryTrait};

use crate::report::Endurance;
use crate::store::StatsStore;

/// `endurance` 单行;avg/longest 未结束会话为 NULL,故经中转 unwrap_or。
#[derive(sea_orm::FromQueryResult)]
struct EnduranceRow {
    /// 会话数。
    sessions: i64,

    /// 平均时长 ms;无会话为 NULL。
    avg_ms: Option<i64>,

    /// 最长时长 ms;无会话为 NULL。
    longest_ms: Option<i64>,
}

impl StatsStore {
    /// 会话续航:窗口内会话数 + 平均 / 最长时长 + 最长连续听歌天数 streak。
    ///
    /// 会话按 `started_at` 落窗;时长 = `ended_at − started_at`(未结束会话按已有 ended 算)。
    /// streak 由 plays 的 UTC 日游程算(gaps-and-islands),与会话窗口同 range。
    ///
    /// # Params:
    ///   - `range`: 时间窗口
    ///
    /// # Return:
    ///   续航聚合;无数据各项为 0
    pub async fn endurance(&self, range: Range<i64>) -> color_eyre::Result<Endurance> {
        let Some(db) = self.pool() else {
            return Ok(Endurance {
                sessions: 0,
                avg_ms: 0,
                longest_ms: 0,
                streak_days: 0,
            });
        };
        let duration = Expr::col((sessions::Entity, sessions::Column::EndedAt))
            .sub(Expr::col((sessions::Entity, sessions::Column::StartedAt)));
        let row = sessions::Entity::find()
            .select_only()
            .column_as(sessions::Column::Id.count(), EnduranceColumn::Sessions)
            .column_as(
                duration.clone().avg().cast_as(Integer),
                EnduranceColumn::AvgMs,
            )
            .column_as(duration.max(), EnduranceColumn::LongestMs)
            .filter(sessions::Column::StartedAt.gte(range.start))
            .filter(sessions::Column::StartedAt.lt(range.end))
            .into_model::<EnduranceRow>()
            .one(db)
            .await
            .wrap_err("endurance 查询失败")?
            .ok_or_else(|| color_eyre::eyre::eyre!("endurance 聚合未返回行"))?;
        let days = plays_in(range)
            .select_only()
            .column_as(
                Expr::col((plays::Entity, plays::Column::StartedAt))
                    .div(1000)
                    .div(86400),
                EnduranceColumn::Day,
            )
            .distinct()
            .into_query();
        let ranked = Query::select()
            .column(EnduranceColumn::Day)
            .expr_window_as(
                Func::cust(RowNumber),
                WindowStatement::new()
                    .order_by(EnduranceColumn::Day, Order::Asc)
                    .to_owned(),
                EnduranceColumn::Rank,
            )
            .from_subquery(days, EnduranceColumn::Days)
            .to_owned();
        let runs = Query::select()
            .expr_as(
                Expr::col(sea_orm::sea_query::Asterisk).count(),
                EnduranceColumn::RunLen,
            )
            .from_subquery(ranked, EnduranceColumn::Ranked)
            .add_group_by([Expr::col(EnduranceColumn::Day).sub(Expr::col(EnduranceColumn::Rank))])
            .to_owned();
        let longest = Query::select()
            .expr(Expr::col(EnduranceColumn::RunLen).max().if_null(0))
            .from_subquery(runs, EnduranceColumn::Runs)
            .to_owned();
        let result = db
            .query_one(&longest)
            .await
            .wrap_err("endurance streak 查询失败")?
            .ok_or_else(|| color_eyre::eyre::eyre!("endurance streak 聚合未返回行"))?;
        Ok(Endurance {
            sessions: row.sessions,
            avg_ms: row.avg_ms.unwrap_or(0),
            longest_ms: row.longest_ms.unwrap_or(0),
            streak_days: result.try_get_by_index(/*index*/ 0)?,
        })
    }
}

/// 会话汇总和连续日期查询使用的列与中间结果名称。
#[derive(Clone, Copy, Debug, sea_orm::DeriveColumn)]
enum EnduranceColumn {
    /// 会话数量。
    Sessions,
    /// 平均会话时长。
    AvgMs,
    /// 最长会话时长。
    LongestMs,
    /// UTC 日期序号。
    Day,
    /// 日期的连续排名。
    Rank,
    /// 连续段长度。
    RunLen,
    /// 去重后的日期集合。
    Days,
    /// 已排名的日期集合。
    Ranked,
    /// 连续日期段集合。
    Runs,
}

/// SQLite 的窗口排名函数。
#[derive(Iden)]
struct RowNumber;

/// SQLite 的整数转换类型。
#[derive(Iden)]
struct Integer;

#[cfg(test)]
mod tests {
    use crate::vocab::FinishReason;

    use super::super::test_support::{DAY, T0, open_temp, play};

    /// endurance:会话时长(ended−started)取平均 / 最长。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn endurance_from_sessions() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        store.touch_session(sid, T0 + 100_000).await?;
        let e = store.endurance(0..i64::MAX).await?;
        assert_eq!(e.sessions, 1);
        assert_eq!(e.longest_ms, 100_000);
        assert_eq!(e.avg_ms, 100_000);
        assert_eq!(e.streak_days, 0, "无 plays → 无连续听歌天数");
        Ok(())
    }

    /// endurance.streak_days:UTC 连续听歌日的最长游程(隔断的日不连成段)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn endurance_streak_counts_longest_consecutive_days() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(T0)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        // 连续 3 天(D0/D1/D2)各一播,跳过 D3,再 D4 一播 → 最长 streak = 3。
        for day in [0, 1, 2, 4] {
            store
                .record_play(&play(
                    "netease",
                    &day.to_string(),
                    T0 + day * DAY,
                    60_000,
                    FinishReason::Eof,
                    None,
                    None,
                    sid,
                ))
                .await?;
        }
        let e = store.endurance(0..i64::MAX).await?;
        assert_eq!(e.streak_days, 3, "D0-D2 连续三天,D3 断开");
        Ok(())
    }
}

//! 会话续航:次数 / 平均 / 最长时长 + 最长连续听歌天数。

use std::ops::Range;

use color_eyre::eyre::WrapErr as _;

use crate::report::Endurance;
use crate::store::StatsStore;

/// `endurance` 单行;avg/longest 未结束会话为 NULL,故经中转 unwrap_or。
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
        let Some(pool) = self.pool() else {
            return Ok(Endurance {
                sessions: 0,
                avg_ms: 0,
                longest_ms: 0,
                streak_days: 0,
            });
        };
        let row = sqlx::query_as!(
            EnduranceRow,
            r#"SELECT COUNT(*) AS "sessions!: i64",
                      CAST(AVG(ended_at - started_at) AS INTEGER) AS "avg_ms?: i64",
                      MAX(ended_at - started_at) AS "longest_ms?: i64"
               FROM sessions WHERE started_at >= ? AND started_at < ?"#,
            range.start,
            range.end
        )
        .fetch_one(pool)
        .await
        .wrap_err("endurance 查询失败")?;
        // 最长连续听歌天数:UTC 日去重后,`day − ROW_NUMBER()` 同值即连续段,取最大段长。
        let streak_days = sqlx::query_scalar!(
            r#"SELECT COALESCE(MAX(run_len), 0) AS "streak!: i64" FROM (
                 SELECT COUNT(*) AS run_len FROM (
                   SELECT day - ROW_NUMBER() OVER (ORDER BY day) AS grp FROM (
                     SELECT DISTINCT started_at / 1000 / 86400 AS day
                     FROM plays WHERE started_at >= ? AND started_at < ?
                   )
                 ) GROUP BY grp
               )"#,
            range.start,
            range.end
        )
        .fetch_one(pool)
        .await
        .wrap_err("endurance streak 查询失败")?;
        Ok(Endurance {
            sessions: row.sessions,
            avg_ms: row.avg_ms.unwrap_or(0),
            longest_ms: row.longest_ms.unwrap_or(0),
            streak_days,
        })
    }
}

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

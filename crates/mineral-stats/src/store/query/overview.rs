//! 播放流水 / 埋点自身状态 / 总量 / 单曲汇总。

use std::ops::Range;

use color_eyre::eyre::WrapErr as _;
use mineral_model::SongId;

use crate::report::{PlayTail, SongSummary, StatusReport, Totals};
use crate::store::StatsStore;
use crate::vocab::FinishReason;

use super::shared::{PlayTailRow, song_id};

/// `status` 主查询单行(events 由各事件表另算,故不直落 StatusReport)。
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
        let Some(pool) = self.pool() else {
            return Ok(Vec::new());
        };
        // source 过滤与否 bind 集不同,天然拆两条编译期查询(同 top_contexts 的 kind 分支)。
        let rows = match source {
            Some(ns) => sqlx::query_as!(
                PlayTailRow,
                r#"SELECT ns, song_value, started_at, listen_ms,
                          finish_reason AS "finish_reason: FinishReason"
                   FROM plays WHERE started_at >= ? AND started_at < ? AND ns = ?
                   ORDER BY started_at DESC LIMIT ?"#,
                range.start,
                range.end,
                ns,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("recent_plays(source) 查询失败")?,
            None => sqlx::query_as!(
                PlayTailRow,
                r#"SELECT ns, song_value, started_at, listen_ms,
                          finish_reason AS "finish_reason: FinishReason"
                   FROM plays WHERE started_at >= ? AND started_at < ?
                   ORDER BY started_at DESC LIMIT ?"#,
                range.start,
                range.end,
                limit
            )
            .fetch_all(pool)
            .await
            .wrap_err("recent_plays 查询失败")?,
        };
        Ok(rows
            .into_iter()
            .map(|r| PlayTail {
                song: song_id(&r.ns, &r.song_value),
                started_at: r.started_at,
                listen_ms: r.listen_ms,
                finish_reason: r.finish_reason,
            })
            .collect())
    }

    /// 埋点系统自身状态:plays / sessions / 全部事件表行数 + 播放时间覆盖。
    pub async fn status(&self) -> color_eyre::Result<StatusReport> {
        let Some(pool) = self.pool() else {
            return Ok(StatusReport {
                plays: 0,
                sessions: 0,
                events: 0,
                first_play_at: None,
                last_play_at: None,
            });
        };
        let row = sqlx::query_as!(
            StatusRow,
            r#"SELECT (SELECT COUNT(*) FROM plays) AS "plays!",
                      (SELECT COUNT(*) FROM sessions) AS "sessions!",
                      (SELECT MIN(started_at) FROM plays) AS "first_play_at?",
                      (SELECT MAX(started_at) FROM plays) AS "last_play_at?""#,
        )
        .fetch_one(pool)
        .await
        .wrap_err("status 查询失败")?;
        let mut events = 0_i64;
        for table in crate::store::prune::EVENT_TABLES {
            let n = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
                .fetch_one(pool)
                .await
                .wrap_err_with(|| format!("status count {table} 失败"))?;
            events += n;
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
        let Some(pool) = self.pool() else {
            return Ok(Totals {
                listen_ms: 0,
                plays: 0,
                completed: 0,
                skipped: 0,
                distinct_songs: 0,
                active_days: 0,
            });
        };
        sqlx::query_as!(
            Totals,
            r#"SELECT
                COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64",
                COUNT(*) AS "plays!: i64",
                COALESCE(SUM(CASE WHEN finish_reason = 'eof' THEN 1 ELSE 0 END), 0) AS "completed!: i64",
                COALESCE(SUM(CASE WHEN finish_reason = 'skip' THEN 1 ELSE 0 END), 0) AS "skipped!: i64",
                COUNT(DISTINCT ns || ':' || song_value) AS "distinct_songs!: i64",
                COUNT(DISTINCT date(started_at / 1000, 'unixepoch')) AS "active_days!: i64"
               FROM plays WHERE started_at >= ? AND started_at < ?"#,
            range.start,
            range.end
        )
        .fetch_one(pool)
        .await
        .wrap_err("totals 查询失败")
    }

    /// 单曲全量汇总(QuerySongStats 改口用);从未播放返回 `None`。
    pub async fn song_summary(&self, id: &SongId) -> color_eyre::Result<Option<SongSummary>> {
        let Some(pool) = self.pool() else {
            return Ok(None);
        };
        let ns = id.namespace().name();
        let value = id.value();
        let row = sqlx::query_as!(
            SongSummary,
            r#"SELECT
                COUNT(*) AS "plays!: i64",
                COALESCE(SUM(CASE WHEN finish_reason = 'skip' THEN 1 ELSE 0 END), 0) AS "skips!: i64",
                COALESCE(SUM(listen_ms), 0) AS "listen_ms!: i64",
                MAX(started_at) AS "last_played_at?: i64"
               FROM plays WHERE ns = ? AND song_value = ?"#,
            ns,
            value
        )
        .fetch_one(pool)
        .await
        .wrap_err("song_summary 查询失败")?;
        if row.plays == 0 {
            return Ok(None);
        }
        Ok(Some(row))
    }
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
        assert_eq!(a.skips, 0);
        assert_eq!(a.listen_ms, 130_000);
        assert_eq!(a.last_played_at, Some(T0 + 14 * HOUR + 60_000));

        let b = store
            .song_summary(&song_id("netease", "2"))
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("B 应有汇总"))?;
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

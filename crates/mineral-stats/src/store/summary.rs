//! full 档事件盘点(`event_summary`):各事件表计数 + 多维分桶。
//!
//! 各分桶是 `(标签, 计数)`;表 / 列是内部常量故分桶走编译期 `query!`,唯 table_counts 因
//! 表名动态走运行期 `query`(与 prune 逐表同理)。全部带时间窗(事件行的 `ts` 列)。

use color_eyre::eyre::WrapErr as _;

use crate::report::{EventCount, EventSummary, Tally};
use crate::store::StatsStore;
use crate::store::prune::EVENT_TABLES;

impl StatsStore {
    /// full 档事件盘点:各事件表计数 + 搜索 / 收藏 / 下载 / 缓存 / 取数 / 动作 / 插件 /
    /// 无缝 / 脚本各维分桶。降级时返回空盘点。
    ///
    /// # Params:
    ///   - `range`: 时间窗口(按事件 `ts`)
    ///   - `limit`: top 类分桶(搜索词 / 下钻页 / 动作)的榜长
    ///
    /// # Return:
    ///   事件盘点
    pub async fn event_summary(
        &self,
        range: std::ops::Range<i64>,
        limit: i64,
    ) -> color_eyre::Result<EventSummary> {
        let Some(pool) = self.pool() else {
            return Ok(EventSummary::default());
        };
        let (lo, hi) = (range.start, range.end);

        // 各事件表行数(表名动态,运行期 query;表名是内部常量非用户输入)。
        let mut table_counts = Vec::<EventCount>::with_capacity(EVENT_TABLES.len());
        for table in EVENT_TABLES {
            let count = sqlx::query_scalar::<_, i64>(&format!(
                "SELECT COUNT(*) FROM {table} WHERE ts >= ? AND ts < ?"
            ))
            .bind(lo)
            .bind(hi)
            .fetch_one(pool)
            .await
            .wrap_err_with(|| format!("event_summary count {table} 失败"))?;
            table_counts.push(EventCount {
                table: table.to_owned(),
                count,
            });
        }

        // top 搜索词:按 query_hash 去重(标签取原文,缺则回落散列)。
        let top_searches = sqlx::query_as!(
            Tally,
            r#"SELECT COALESCE(query, query_hash) AS "label!: String", COUNT(*) AS "count!: i64"
               FROM searches WHERE ts >= ? AND ts < ?
               GROUP BY COALESCE(query, query_hash) ORDER BY 2 DESC LIMIT ?"#,
            lo,
            hi,
            limit
        )
        .fetch_all(pool)
        .await
        .wrap_err("event_summary top_searches 失败")?;

        // love 新增按 origin 分桶(仅 loved=1)。
        let love_by_origin = sqlx::query_as!(
            Tally,
            r#"SELECT origin AS "label!", COUNT(*) AS "count!: i64"
               FROM love_changes WHERE ts >= ? AND ts < ? AND loved = 1
               GROUP BY origin ORDER BY 2 DESC"#,
            lo,
            hi
        )
        .fetch_all(pool)
        .await
        .wrap_err("event_summary love_by_origin 失败")?;

        // 下载三态。
        let downloads_by_outcome = sqlx::query_as!(
            Tally,
            r#"SELECT outcome AS "label!", COUNT(*) AS "count!: i64"
               FROM downloads WHERE ts >= ? AND ts < ?
               GROUP BY outcome ORDER BY 2 DESC"#,
            lo,
            hi
        )
        .fetch_all(pool)
        .await
        .wrap_err("event_summary downloads_by_outcome 失败")?;

        // 缓存收割。
        let harvests_by_outcome = sqlx::query_as!(
            Tally,
            r#"SELECT outcome AS "label!", COUNT(*) AS "count!: i64"
               FROM cache_harvests WHERE ts >= ? AND ts < ?
               GROUP BY outcome ORDER BY 2 DESC"#,
            lo,
            hi
        )
        .fetch_all(pool)
        .await
        .wrap_err("event_summary harvests_by_outcome 失败")?;

        // top 下钻页(fetch_kind)。
        let top_fetches = sqlx::query_as!(
            Tally,
            r#"SELECT fetch_kind AS "label!", COUNT(*) AS "count!: i64"
               FROM fetches WHERE ts >= ? AND ts < ?
               GROUP BY fetch_kind ORDER BY 2 DESC LIMIT ?"#,
            lo,
            hi,
            limit
        )
        .fetch_all(pool)
        .await
        .wrap_err("event_summary top_fetches 失败")?;

        // top 具名动作。
        let top_actions = sqlx::query_as!(
            Tally,
            r#"SELECT name AS "label!", COUNT(*) AS "count!: i64"
               FROM action_invocations WHERE ts >= ? AND ts < ?
               GROUP BY name ORDER BY 2 DESC LIMIT ?"#,
            lo,
            hi,
            limit
        )
        .fetch_all(pool)
        .await
        .wrap_err("event_summary top_actions 失败")?;

        // 补救漏斗:hook_fires 按 decision。
        let hooks_by_decision = sqlx::query_as!(
            Tally,
            r#"SELECT decision AS "label!", COUNT(*) AS "count!: i64"
               FROM hook_fires WHERE ts >= ? AND ts < ?
               GROUP BY decision ORDER BY 2 DESC"#,
            lo,
            hi
        )
        .fetch_all(pool)
        .await
        .wrap_err("event_summary hooks_by_decision 失败")?;

        // 无缝率:gapless_boundaries 按 result。
        let gapless_by_result = sqlx::query_as!(
            Tally,
            r#"SELECT result AS "label!", COUNT(*) AS "count!: i64"
               FROM gapless_boundaries WHERE ts >= ? AND ts < ?
               GROUP BY result ORDER BY 2 DESC"#,
            lo,
            hi
        )
        .fetch_all(pool)
        .await
        .wrap_err("event_summary gapless_by_result 失败")?;

        // 脚本健康:script_lifecycle 按 event。
        let script_by_event = sqlx::query_as!(
            Tally,
            r#"SELECT event AS "label!", COUNT(*) AS "count!: i64"
               FROM script_lifecycle WHERE ts >= ? AND ts < ?
               GROUP BY event ORDER BY 2 DESC"#,
            lo,
            hi
        )
        .fetch_all(pool)
        .await
        .wrap_err("event_summary script_by_event 失败")?;

        Ok(EventSummary {
            table_counts,
            top_searches,
            love_by_origin,
            downloads_by_outcome,
            harvests_by_outcome,
            top_fetches,
            top_actions,
            hooks_by_decision,
            gapless_by_result,
            script_by_event,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::event::{BehaviorEvent, LoveOrigin, StatsEvent};
    use crate::store::StatsStore;
    use crate::vocab::Actor;
    use mineral_model::{SongId, SourceKind};

    async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    /// event_summary:空库列全 28 张表且计数全 0,各分桶空。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn event_summary_lists_all_tables_empty() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let s = store.event_summary(0..i64::MAX, 10).await?;
        assert_eq!(s.table_counts.len(), 28, "28 张事件表");
        assert!(s.table_counts.iter().all(|e| e.count == 0), "空库全 0");
        assert!(s.table_counts.iter().any(|e| e.table == "searches"));
        assert!(s.love_by_origin.is_empty(), "空库无分桶");
        Ok(())
    }

    /// love 新增按 origin 分桶:只数 loved=true,user / import 各归桶。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn event_summary_buckets_love_by_origin() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let song = SongId::new(SourceKind::NETEASE, "1");
        let love = |loved, origin| StatsEvent::Behavior {
            actor: Actor::User,
            event: BehaviorEvent::LoveChange {
                song: song.clone(),
                loved,
                origin,
                remote_mirror: None,
            },
        };
        // user 收藏 ×2、import 收藏 ×1、一次取消收藏(loved=false,不计)。
        store
            .record_event(1000, None, &love(true, LoveOrigin::User))
            .await?;
        store
            .record_event(2000, None, &love(true, LoveOrigin::User))
            .await?;
        store
            .record_event(3000, None, &love(true, LoveOrigin::Import))
            .await?;
        store
            .record_event(4000, None, &love(false, LoveOrigin::User))
            .await?;
        let s = store.event_summary(0..i64::MAX, 10).await?;
        let user = s
            .love_by_origin
            .iter()
            .find(|t| t.label == "user")
            .ok_or_else(|| color_eyre::eyre::eyre!("无 user 桶"))?;
        assert_eq!(user.count, 2, "user 新增 2(取消那次不计)");
        let import = s
            .love_by_origin
            .iter()
            .find(|t| t.label == "import")
            .ok_or_else(|| color_eyre::eyre::eyre!("无 import 桶"))?;
        assert_eq!(import.count, 1);
        Ok(())
    }
}

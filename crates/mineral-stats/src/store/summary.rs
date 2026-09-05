//! full 档事件盘点(`event_summary`):各事件表计数 + 多维分桶。
//!
//! 各分桶以实体字段指定标签与时间窗口，事件集合统一参与盘点。

use crate::entity::{
    action_invocations, cache_harvests, downloads, fetches, gapless_boundaries, hook_fires,
    love_changes, script_lifecycle, searches,
};
use color_eyre::eyre::WrapErr as _;
use sea_orm::sea_query::{Expr, ExprTrait};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, QuerySelect, Select};

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
        let Some(db) = self.pool() else {
            return Ok(EventSummary::default());
        };
        let mut table_counts = Vec::with_capacity(EVENT_TABLES.len());
        for table in EVENT_TABLES {
            table_counts.push(EventCount {
                table: table.name().to_owned(),
                count: table
                    .count(db, Some(range.clone()))
                    .await
                    .wrap_err_with(|| format!("event_summary count {} 失败", table.name()))?,
            });
        }
        let top_searches = tally::<searches::Entity>(
            Expr::col((searches::Entity, searches::Column::Query))
                .if_null(Expr::col((searches::Entity, searches::Column::QueryHash))),
            searches::Column::Id,
            searches::Column::Ts,
            range.clone(),
            Some(limit),
        )
        .into_model::<Tally>()
        .all(db)
        .await
        .wrap_err("event_summary top_searches 失败")?;
        let love_by_origin = tally::<love_changes::Entity>(
            Expr::col((love_changes::Entity, love_changes::Column::Origin)),
            love_changes::Column::Id,
            love_changes::Column::Ts,
            range.clone(),
            /*limit*/ None,
        )
        .filter(love_changes::Column::Loved.eq(1))
        .into_model::<Tally>()
        .all(db)
        .await
        .wrap_err("event_summary love_by_origin 失败")?;
        let downloads_by_outcome = tally::<downloads::Entity>(
            Expr::col((downloads::Entity, downloads::Column::Outcome)),
            downloads::Column::Id,
            downloads::Column::Ts,
            range.clone(),
            /*limit*/ None,
        )
        .into_model::<Tally>()
        .all(db)
        .await
        .wrap_err("event_summary downloads_by_outcome 失败")?;
        let harvests_by_outcome = tally::<cache_harvests::Entity>(
            Expr::col((cache_harvests::Entity, cache_harvests::Column::Outcome)),
            cache_harvests::Column::Id,
            cache_harvests::Column::Ts,
            range.clone(),
            /*limit*/ None,
        )
        .into_model::<Tally>()
        .all(db)
        .await
        .wrap_err("event_summary harvests_by_outcome 失败")?;
        let top_fetches = tally::<fetches::Entity>(
            Expr::col((fetches::Entity, fetches::Column::FetchKind)),
            fetches::Column::Id,
            fetches::Column::Ts,
            range.clone(),
            Some(limit),
        )
        .into_model::<Tally>()
        .all(db)
        .await
        .wrap_err("event_summary top_fetches 失败")?;
        let top_actions = tally::<action_invocations::Entity>(
            Expr::col((action_invocations::Entity, action_invocations::Column::Name)),
            action_invocations::Column::Id,
            action_invocations::Column::Ts,
            range.clone(),
            Some(limit),
        )
        .into_model::<Tally>()
        .all(db)
        .await
        .wrap_err("event_summary top_actions 失败")?;
        let hooks_by_decision = tally::<hook_fires::Entity>(
            Expr::col((hook_fires::Entity, hook_fires::Column::Decision)),
            hook_fires::Column::Id,
            hook_fires::Column::Ts,
            range.clone(),
            /*limit*/ None,
        )
        .into_model::<Tally>()
        .all(db)
        .await
        .wrap_err("event_summary hooks_by_decision 失败")?;
        let gapless_by_result = tally::<gapless_boundaries::Entity>(
            Expr::col((
                gapless_boundaries::Entity,
                gapless_boundaries::Column::Result,
            )),
            gapless_boundaries::Column::Id,
            gapless_boundaries::Column::Ts,
            range.clone(),
            /*limit*/ None,
        )
        .into_model::<Tally>()
        .all(db)
        .await
        .wrap_err("event_summary gapless_by_result 失败")?;
        let script_by_event = tally::<script_lifecycle::Entity>(
            Expr::col((script_lifecycle::Entity, script_lifecycle::Column::Event)),
            script_lifecycle::Column::Id,
            script_lifecycle::Column::Ts,
            range.clone(),
            /*limit*/ None,
        )
        .into_model::<Tally>()
        .all(db)
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

/// 标签分桶的输出列。
#[derive(Clone, Copy, Debug, sea_orm::DeriveColumn)]
enum TallyColumn {
    /// 分桶标签。
    Label,
    /// 事件数量。
    Count,
}

/// 按实体字段分桶；负数榜长沿用 SQLite 不限条数语义。
fn tally<E: EntityTrait>(
    label: Expr,
    id: E::Column,
    timestamp: E::Column,
    range: std::ops::Range<i64>,
    limit: Option<i64>,
) -> Select<E> {
    E::find()
        .select_only()
        .column_as(label.clone(), TallyColumn::Label)
        .column_as(id.count(), TallyColumn::Count)
        .filter(timestamp.gte(range.start))
        .filter(timestamp.lt(range.end))
        .group_by(label)
        .order_by_desc(Expr::col(TallyColumn::Count))
        .limit(limit.and_then(|limit| u64::try_from(limit).ok()))
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

    /// event_summary:空库列全事件表且计数全 0,各分桶空。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn event_summary_lists_all_tables_empty() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let s = store.event_summary(0..i64::MAX, 10).await?;
        assert_eq!(
            s.table_counts.len(),
            crate::store::prune::EVENT_TABLES.len(),
            "全部事件表"
        );
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

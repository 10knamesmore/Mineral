//! retention:按时间裁剪 stats.db 的旧流水。

use color_eyre::eyre::WrapErr as _;

use crate::store::StatsStore;

/// 所有按 `ts` 裁剪的事件表。**单一真相源**:新增事件表必须补进来,由完整性测试
/// `prune_registry_matches_migration` 兜住漏项(漏则测试红,而非静默漏删)。plays /
/// sessions 不在此(裁剪列不同:started_at / ended_at,单独 query 处理);songs 维表
/// 不在此(维度非流水,任意年代的事实行都要 JOIN 它出名,永不裁剪)。
pub(crate) const EVENT_TABLES: &[&str] = &[
    "searches",
    "seeks",
    "pauses",
    "volume_changes",
    "mode_changes",
    "love_changes",
    "queue_ops",
    "playlist_ops",
    "fetches",
    "downloads",
    "task_cancels",
    "copy_renders",
    "action_invocations",
    "config_overrides",
    "store_writes",
    "spawns",
    "bus_messages",
    "fullscreen_changes",
    "connection_rejects",
    "client_connections",
    "app_lifecycle",
    "url_resolutions",
    "hook_fires",
    "gapless_boundaries",
    "prefetches",
    "cache_harvests",
    "cache_evictions",
    "script_lifecycle",
    "config_reloads",
];

/// 某名是否是合法的全谱事件 kind(= 事件表名)。config `stats.collect` 校验未知 kind 用:
/// `plays` / `sessions` 是 core 本体、不在此集(它们的开关由 `level` 定,不能经 collect 覆盖)。
///
/// # Params:
///   - `name`: 待判定的 kind 名
///
/// # Return:
///   是事件表之一返回 `true`
pub fn is_event_kind(name: &str) -> bool {
    EVENT_TABLES.contains(&name)
}

impl StatsStore {
    /// 裁掉 `before_ms` 之前的流水:plays + 全部事件表 + 已无子行引用的旧会话。降级时 no-op。
    ///
    /// 删除顺序 plays → 事件表 → sessions。sessions **不能只按 `ended_at` 判**:
    /// `ended_at` 只随播放活动推进(`record_event` 不 touch),会话以事件收尾时子行
    /// `ts > ended_at`,仅按时间删父行会撞外键、整轮事务回滚,retention 从此停摆——
    /// 故叠加「无存活引用」守卫,被引用的旧会话留到其子行也进水位再删(自愈)。
    /// 整体包一个事务。
    ///
    /// # Params:
    ///   - `before_ms`: 裁剪水位;严格早于此的行被删
    pub async fn prune(&self, before_ms: i64) -> color_eyre::Result<()> {
        let Some(pool) = self.pool() else {
            return Ok(());
        };
        let mut tx = pool.begin().await.wrap_err("prune 事务开启失败")?;
        sqlx::query!("DELETE FROM plays WHERE started_at < ?", before_ms)
            .execute(&mut *tx)
            .await
            .wrap_err("prune plays 失败")?;
        for table in EVENT_TABLES {
            // 表名是内部常量(非用户输入),值走 bind;事件表统一 ts 列。
            sqlx::query(&format!("DELETE FROM {table} WHERE ts < ?"))
                .bind(before_ms)
                .execute(&mut *tx)
                .await
                .wrap_err_with(|| format!("prune {table} 失败"))?;
        }
        sqlx::query(&format!(
            "DELETE FROM sessions WHERE ended_at < ? AND {}",
            unreferenced_session_guard()
        ))
        .bind(before_ms)
        .execute(&mut *tx)
        .await
        .wrap_err("prune sessions 失败")?;
        tx.commit().await.wrap_err("prune 提交失败")?;
        Ok(())
    }

    /// dry-run 计数:一次 [`Self::prune`] 会删掉的总行数(plays + 全部事件表 + 旧会话)。
    /// 供 CLI `prune` 无 `--yes` 时预告删量,不动盘。降级返回 0。
    ///
    /// 会话计数按删除时序模拟:先数将删的 plays / 事件行,再以「删净水位内子行后」的
    /// 口径数 sessions——引用守卫只看 `ts >= 水位` 的存活子行,与 [`Self::prune`] 实删
    /// 一致(prune 时水位内子行已在同事务内先删)。
    ///
    /// # Params:
    ///   - `before_ms`: 裁剪水位;严格早于此的行计入
    ///
    /// # Return:
    ///   将被删除的总行数
    pub async fn count_before(&self, before_ms: i64) -> color_eyre::Result<i64> {
        let Some(pool) = self.pool() else {
            return Ok(0);
        };
        let mut total = sqlx::query_scalar!(
            r#"SELECT COUNT(*) AS "n!: i64" FROM plays WHERE started_at < ?"#,
            before_ms
        )
        .fetch_one(pool)
        .await
        .wrap_err("count_before plays 失败")?;
        for table in EVENT_TABLES {
            // 表名是内部常量(非用户输入);事件表统一 ts 列。
            total +=
                sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table} WHERE ts < ?"))
                    .bind(before_ms)
                    .fetch_one(pool)
                    .await
                    .wrap_err_with(|| format!("count_before {table} 失败"))?;
        }
        total += sqlx::query_scalar::<_, i64>(&format!(
            "SELECT COUNT(*) FROM sessions WHERE ended_at < ?1 AND {}",
            surviving_reference_guard()
        ))
        .bind(before_ms)
        .fetch_one(pool)
        .await
        .wrap_err("count_before sessions 失败")?;
        Ok(total)
    }
}

/// sessions 删除守卫:全部子表(plays + 事件表)均无引用本会话的行。表名全为内部
/// 常量,拼接安全;引用列统一叫 `session_id`(migration 约定)。
fn unreferenced_session_guard() -> String {
    std::iter::once("plays")
        .chain(EVENT_TABLES.iter().copied())
        .map(|table| format!("NOT EXISTS (SELECT 1 FROM {table} WHERE session_id = sessions.id)"))
        .collect::<Vec<String>>()
        .join(" AND ")
}

/// dry-run 版守卫:模拟「水位内子行已删」后的引用判定——只把 `ts`(plays 为
/// `started_at`)不早于水位的行算作存活引用,与实删口径一致。复用外层的水位 bind
/// (SQLite 的 `?` 按位置复用需重复出现,这里每张表引用同一水位值,统一用 `?1`)。
fn surviving_reference_guard() -> String {
    std::iter::once(("plays", "started_at"))
        .chain(EVENT_TABLES.iter().map(|&table| (table, "ts")))
        .map(|(table, ts_column)| {
            format!(
                "NOT EXISTS (SELECT 1 FROM {table} WHERE session_id = sessions.id AND {ts_column} >= ?1)"
            )
        })
        .collect::<Vec<String>>()
        .join(" AND ")
}

#[cfg(test)]
mod tests {
    use super::EVENT_TABLES;
    use crate::context::QueueContext;
    use crate::event::{BehaviorEvent, SearchOutcome, SearchTargetKind, StatsEvent};
    use crate::play::PlayRecord;
    use crate::store::StatsStore;
    use crate::vocab::{Actor, FinishReason, PlayOrigin, PlaybackOrigin};
    use color_eyre::eyre::WrapErr as _;
    use mineral_model::{SongId, SourceKind};
    use rustc_hash::FxHashSet;
    use sqlx::SqlitePool;

    async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    fn live(store: &StatsStore) -> color_eyre::Result<&SqlitePool> {
        store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("期望 live pool"))
    }

    /// 造一行播放事实(裁剪测试只关心 started_at / session_id)。
    fn play_at(started_at: i64, session_id: i64) -> PlayRecord {
        PlayRecord {
            song_id: SongId::new(SourceKind::NETEASE, "1"),
            started_at,
            ended_at: started_at,
            listen_ms: 0,
            duration_ms_snapshot: None,
            finish_reason: FinishReason::Eof,
            skip_at_ms: None,
            play_mode: crate::PlayMode::Sequential,
            session_id,
            origin: PlayOrigin::Explicit,
            actor: Actor::User,
            context: QueueContext::Unknown,
            audio: crate::PlayAudioSnapshot::default(),
            playback_origin: PlaybackOrigin::Remote,
        }
    }

    /// 造一条搜索事件(裁剪测试只关心 ts)。
    fn search_event() -> StatsEvent {
        StatsEvent::Behavior {
            actor: Actor::User,
            event: BehaviorEvent::Search {
                query: None,
                query_hash: "h".to_owned(),
                kind: SearchTargetKind::Song,
                source: SourceKind::NETEASE,
                page: 0,
                result_count: None,
                outcome: SearchOutcome::Ok,
            },
        }
    }

    async fn count(pool: &SqlitePool, table: &str) -> color_eyre::Result<i64> {
        sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(pool)
            .await
            .wrap_err_with(|| format!("count {table} 失败"))
    }

    #[tokio::test]
    async fn prune_registry_matches_migration() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let pool = live(&store)?;
        let db_tables = sqlx::query_scalar::<_, String>(
            "SELECT name FROM sqlite_master WHERE type = 'table' \
             AND name NOT IN ('plays', 'sessions', 'songs', '_sqlx_migrations') \
             AND name NOT LIKE 'sqlite_%'",
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect::<FxHashSet<String>>();
        let registry = EVENT_TABLES
            .iter()
            .map(|s| (*s).to_owned())
            .collect::<FxHashSet<String>>();
        assert_eq!(
            registry, db_tables,
            "prune EVENT_TABLES 须与 migration 事件表严格一致"
        );
        Ok(())
    }

    #[tokio::test]
    async fn prune_deletes_old_keeps_new() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let (old_ts, new_ts, cutoff) = (1000_i64, 100_000_i64, 50_000_i64);

        let sid_old = store
            .open_session(old_ts)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        let sid_new = store
            .open_session(new_ts)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        store.record_play(&play_at(old_ts, sid_old)).await?;
        store.record_play(&play_at(new_ts, sid_new)).await?;
        store
            .record_event(old_ts, Some(sid_old), &search_event())
            .await?;
        store
            .record_event(new_ts, Some(sid_new), &search_event())
            .await?;

        let pool = live(&store)?;
        assert_eq!(count(pool, "plays").await?, 2);
        assert_eq!(count(pool, "searches").await?, 2);
        assert_eq!(count(pool, "sessions").await?, 2);

        store.prune(cutoff).await?;

        assert_eq!(count(pool, "plays").await?, 1, "旧 play 删、新 play 留");
        assert_eq!(count(pool, "searches").await?, 1, "旧搜索删、新搜索留");
        assert_eq!(count(pool, "sessions").await?, 1, "旧空会话删、新会话留");
        Ok(())
    }

    #[tokio::test]
    async fn count_before_matches_prune_deletion() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let (old_ts, new_ts, cutoff) = (1000_i64, 100_000_i64, 50_000_i64);
        let sid_old = store
            .open_session(old_ts)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        let sid_new = store
            .open_session(new_ts)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        store.record_play(&play_at(old_ts, sid_old)).await?;
        store.record_play(&play_at(new_ts, sid_new)).await?;
        store
            .record_event(old_ts, Some(sid_old), &search_event())
            .await?;
        store
            .record_event(new_ts, Some(sid_new), &search_event())
            .await?;

        // 旧 play + 旧 search + 旧空会话 = 3 行将删(新的都留)。
        assert_eq!(store.count_before(cutoff).await?, 3, "预告删量 = 旧行数");

        // dry-run 不动盘:计数后行数不变。
        let pool = live(&store)?;
        assert_eq!(count(pool, "plays").await?, 2, "count_before 不删行");
        Ok(())
    }

    #[tokio::test]
    async fn count_before_disabled_is_zero() -> color_eyre::Result<()> {
        assert_eq!(StatsStore::disabled().count_before(1000).await?, 0);
        Ok(())
    }

    /// straddle 回归:会话以事件收尾(事件 ts > 会话 ended_at,record_event 不 touch
    /// ended_at),水位落在两者之间时,prune 不得因删父行撞外键回滚——被引用的旧会话
    /// 留下,其余照删;水位推进过事件后该会话再被删(自愈)。
    #[tokio::test]
    async fn prune_keeps_session_referenced_by_later_event() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        // 会话最后播放在 T1=1000(ended_at 停此),同会话搜索事件在 T2=80_000,水位 50_000。
        let (t1, t2, cutoff) = (1000_i64, 80_000_i64, 50_000_i64);
        let sid = store
            .open_session(t1)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("session"))?;
        store.record_play(&play_at(t1, sid)).await?;
        store.record_event(t2, Some(sid), &search_event()).await?;

        // dry-run 与实删同口径:旧 play 1 行删;会话被 ts=T2 存活事件引用,不计入。
        assert_eq!(store.count_before(cutoff).await?, 1, "预告删量不含被引会话");

        store.prune(cutoff).await.wrap_err("straddle 不应回滚")?;
        let pool = live(&store)?;
        assert_eq!(count(pool, "plays").await?, 0, "旧 play 删净");
        assert_eq!(count(pool, "searches").await?, 1, "水位后的事件保留");
        assert_eq!(count(pool, "sessions").await?, 1, "被引用会话保留,不撞外键");

        // 水位推进过 T2:事件删净后会话失去引用,本轮一并删——自愈,不留孤儿。
        store.prune(t2 + 1).await?;
        assert_eq!(count(pool, "searches").await?, 0);
        assert_eq!(count(pool, "sessions").await?, 0, "失去引用后会话补删");
        Ok(())
    }

    /// is_event_kind:事件表名判 true;core 本体(plays / sessions)与未知名判 false。
    #[test]
    fn is_event_kind_classifies_names() {
        assert!(super::is_event_kind("searches"));
        assert!(super::is_event_kind("gapless_boundaries"));
        assert!(!super::is_event_kind("plays"), "core 本体不是事件 kind");
        assert!(!super::is_event_kind("sessions"), "core 本体不是事件 kind");
        assert!(!super::is_event_kind("nope_typo"), "未知名");
        // 与登记表一致:每张 EVENT_TABLE 都判 true。
        assert!(super::EVENT_TABLES.iter().all(|t| super::is_event_kind(t)));
    }

    #[tokio::test]
    async fn prune_disabled_is_noop() -> color_eyre::Result<()> {
        StatsStore::disabled().prune(1000).await
    }
}

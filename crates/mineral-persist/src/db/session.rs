//! 全局会话存储。

use color_eyre::eyre::WrapErr;
use mineral_log::debug;
use mineral_model::{SongId, SourceKind};

use crate::Persist;
use crate::db::time::now_ms;

/// `session_state` 单例行的列投影:`(cur_namespace, cur_song_value, position_ms, play_mode, volume)`。
type SessionStateRow = (Option<String>, Option<String>, i64, String, f64);

/// 会话快照：重启恢复"上次听到哪"。队列可跨 namespace。
#[derive(Debug, Clone)]
pub struct SessionSnapshot {
    /// 当前歌(空队列为 None)。
    pub current: Option<SongId>,

    /// 当前播放位置毫秒。
    pub position_ms: u64,

    /// 播放模式(枚举名稳定串，由 server 侧 PlayMode 落地)。
    pub play_mode: String,

    /// 音量 0.0..=1.0。
    pub volume: f64,

    /// 队列(保序)。
    pub queue: Vec<SongId>,
}

/// 全局会话存储(单例行 id=0)。
pub struct SessionStore {
    /// 顶层句柄。
    persist: Persist,
}

impl SessionStore {
    /// 构造。
    ///
    /// # Params:
    ///   - `persist`: 顶层句柄
    pub(crate) fn new(persist: Persist) -> Self {
        Self { persist }
    }

    /// 保存会话(覆盖单例 + 重写队列表)。降级静默成功。
    ///
    /// # Params:
    ///   - `snap`: 待保存的会话快照
    ///
    /// # Return:
    ///   成功返回 `Ok(())`；降级(无 pool)也静默成功。
    pub async fn save(&self, snap: &SessionSnapshot) -> color_eyre::Result<()> {
        let Some(pool) = self.persist.pool() else {
            return Ok(());
        };
        debug!(target: "persist", queue_len = snap.queue.len(), "保存会话");
        let pos = i64::try_from(snap.position_ms)?;
        let (cur_ns, cur_val): (Option<&str>, Option<String>) = match &snap.current {
            Some(id) => (Some(id.namespace().name()), Some(id.value().to_owned())),
            None => (None, None),
        };
        // 多步写(session_state upsert + session_queue 先清后插)包进事务原子完成,
        // 否则并发 save(状态变化 + 15s 节流可能同时触发)交错会撞 session_queue 主键。
        let mut tx = pool.begin().await.wrap_err("开启 save 会话事务失败")?;
        sqlx::query(
            "INSERT INTO session_state(id,cur_namespace,cur_song_value,position_ms,play_mode,volume,updated_at)
             VALUES(0,?,?,?,?,?,?)
             ON CONFLICT(id) DO UPDATE SET
               cur_namespace=excluded.cur_namespace, cur_song_value=excluded.cur_song_value,
               position_ms=excluded.position_ms, play_mode=excluded.play_mode,
               volume=excluded.volume, updated_at=excluded.updated_at",
        )
        .bind(cur_ns).bind(cur_val).bind(pos).bind(&snap.play_mode).bind(snap.volume).bind(now_ms())
        .execute(&mut *tx).await
        .wrap_err("保存会话状态(session_state)失败")?;
        sqlx::query("DELETE FROM session_queue")
            .execute(&mut *tx)
            .await
            .wrap_err("清空会话队列(session_queue)失败")?;
        for (i, id) in snap.queue.iter().enumerate() {
            let p = i64::try_from(i)?;
            sqlx::query("INSERT INTO session_queue(position,namespace,song_value) VALUES(?,?,?)")
                .bind(p)
                .bind(id.namespace().name())
                .bind(id.value())
                .execute(&mut *tx)
                .await
                .wrap_err_with(|| format!("写入会话队列项失败 position={p}"))?;
        }
        tx.commit().await.wrap_err("提交 save 会话事务失败")?;
        Ok(())
    }

    /// 读会话。降级 / 无会话返回 `None`。
    ///
    /// # Return:
    ///   命中返回完整会话(队列按 position 升序重建)，否则 None。
    pub async fn load(&self) -> color_eyre::Result<Option<SessionSnapshot>> {
        let Some(pool) = self.persist.pool() else {
            return Ok(None);
        };
        let head: Option<SessionStateRow> = sqlx::query_as(
            "SELECT cur_namespace,cur_song_value,position_ms,play_mode,volume FROM session_state WHERE id=0",
        )
        .fetch_optional(pool).await
        .wrap_err("读会话状态(session_state)失败")?;
        let Some((cur_ns, cur_val, pos, play_mode, volume)) = head else {
            return Ok(None);
        };
        let current = match (cur_ns, cur_val) {
            (Some(ns), Some(v)) => Some(SongId::new(SourceKind::from_name(&ns), v)),
            _ => None,
        };
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT namespace,song_value FROM session_queue ORDER BY position")
                .fetch_all(pool)
                .await
                .wrap_err("读会话队列(session_queue)失败")?;
        let queue = rows
            .into_iter()
            .map(|(ns, v)| SongId::new(SourceKind::from_name(&ns), v))
            .collect::<Vec<SongId>>();
        Ok(Some(SessionSnapshot {
            current,
            position_ms: u64::try_from(pos)?,
            play_mode,
            volume,
            queue,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn session_save_load_roundtrip() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::Persist::open(&dir.path().join("t.db")).await?;
        let snap = SessionSnapshot {
            current: Some(SongId::new(SourceKind::NETEASE, "123")),
            position_ms: 42_000,
            play_mode: "shuffle".to_owned(),
            volume: 0.8,
            queue: vec![
                SongId::new(SourceKind::NETEASE, "123"),
                SongId::new(SourceKind::LOCAL, "abc"),
            ],
        };
        p.session().save(&snap).await?;
        let back = p.session().load().await?;
        assert!(back.is_some());
        if let Some(back) = back {
            assert_eq!(back.queue.len(), 2);
            assert_eq!(back.position_ms, 42_000);
            assert_eq!(back.play_mode, "shuffle");
            // 跨 namespace 还原正确
            let Some(second) = back.queue.get(1) else {
                return Err(color_eyre::eyre::eyre!("queue missing second"));
            };
            assert_eq!(second.namespace(), SourceKind::LOCAL);
            assert!(back.current.is_some());
        }
        Ok(())
    }

    #[tokio::test]
    async fn session_load_empty_returns_none() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::Persist::open(&dir.path().join("t.db")).await?;
        assert!(p.session().load().await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn session_save_overwrites_queue() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::Persist::open(&dir.path().join("t.db")).await?;
        let s1 = SessionSnapshot {
            current: None,
            position_ms: 0,
            play_mode: "loop".to_owned(),
            volume: 1.0,
            queue: vec![
                SongId::new(SourceKind::NETEASE, "a"),
                SongId::new(SourceKind::NETEASE, "b"),
            ],
        };
        p.session().save(&s1).await?;
        let s2 = SessionSnapshot {
            current: None,
            position_ms: 0,
            play_mode: "loop".to_owned(),
            volume: 1.0,
            queue: vec![SongId::new(SourceKind::NETEASE, "c")],
        };
        p.session().save(&s2).await?;
        let back = p.session().load().await?;
        assert!(back.is_some());
        if let Some(back) = back {
            assert_eq!(back.queue.len(), 1); // 旧队列被清
        }
        Ok(())
    }
}

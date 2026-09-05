//! 全局会话存储。

use crate::entity::{session_queue, session_state};
use color_eyre::eyre::WrapErr;
use mineral_log::debug;
use mineral_model::{SongId, SourceKind};
use sea_orm::sea_query::OnConflict;
use sea_orm::{EntityTrait, QueryOrder, Set, TransactionTrait};

use crate::ServerStore;
use crate::db::time::now_ms;

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
    persist: ServerStore,
}

impl SessionStore {
    /// 构造。
    ///
    /// # Params:
    ///   - `persist`: 顶层句柄
    pub(crate) fn new(persist: ServerStore) -> Self {
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
        let Some(db) = self.persist.pool() else {
            return Ok(());
        };
        debug!(target: "persist", queue_len = snap.queue.len(), "保存会话");
        let tx = db.begin().await.wrap_err("开启 save 会话事务失败")?;
        session_state::Entity::insert(session_state::ActiveModel {
            id: Set(0),
            cur_namespace: Set(snap
                .current
                .as_ref()
                .map(|id| id.namespace().name().to_owned())),
            cur_song_value: Set(snap.current.as_ref().map(|id| id.value().to_owned())),
            position_ms: Set(i64::try_from(snap.position_ms)?),
            play_mode: Set(snap.play_mode.clone()),
            volume: Set(snap.volume),
            updated_at: Set(now_ms()),
        })
        .on_conflict(
            OnConflict::column(session_state::Column::Id)
                .update_columns([
                    session_state::Column::CurNamespace,
                    session_state::Column::CurSongValue,
                    session_state::Column::PositionMs,
                    session_state::Column::PlayMode,
                    session_state::Column::Volume,
                    session_state::Column::UpdatedAt,
                ])
                .to_owned(),
        )
        .exec_without_returning(&tx)
        .await
        .wrap_err("保存会话状态(session_state)失败")?;
        session_queue::Entity::delete_many()
            .exec(&tx)
            .await
            .wrap_err("清空会话队列(session_queue)失败")?;
        for (index, id) in snap.queue.iter().enumerate() {
            session_queue::Entity::insert(session_queue::ActiveModel {
                position: Set(i64::try_from(index)?),
                namespace: Set(id.namespace().name().to_owned()),
                song_value: Set(id.value().to_owned()),
            })
            .exec_without_returning(&tx)
            .await
            .wrap_err_with(|| format!("写入会话队列项失败 position={index}"))?;
        }
        tx.commit().await.wrap_err("提交 save 会话事务失败")?;
        Ok(())
    }

    /// 读会话。降级 / 无会话返回 `None`。
    ///
    /// # Return:
    ///   命中返回完整会话(队列按 position 升序重建)，否则 None。
    pub async fn load(&self) -> color_eyre::Result<Option<SessionSnapshot>> {
        let Some(db) = self.persist.pool() else {
            return Ok(None);
        };
        let Some(head) = session_state::Entity::find_by_id(0)
            .one(db)
            .await
            .wrap_err("读会话状态(session_state)失败")?
        else {
            return Ok(None);
        };
        let current = match (head.cur_namespace, head.cur_song_value) {
            (Some(ns), Some(value)) => Some(SongId::new(SourceKind::from_name(&ns), value)),
            _ => None,
        };
        let rows = session_queue::Entity::find()
            .order_by_asc(session_queue::Column::Position)
            .all(db)
            .await
            .wrap_err("读会话队列(session_queue)失败")?;
        let queue = rows
            .into_iter()
            .map(|row| SongId::new(SourceKind::from_name(&row.namespace), row.song_value))
            .collect();
        Ok(Some(SessionSnapshot {
            current,
            position_ms: u64::try_from(head.position_ms)?,
            play_mode: head.play_mode,
            volume: head.volume,
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
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
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
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
        assert!(p.session().load().await?.is_none());
        Ok(())
    }

    #[tokio::test]
    async fn session_save_overwrites_queue() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let p = crate::ServerStore::open(&dir.path().join("t.db")).await?;
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

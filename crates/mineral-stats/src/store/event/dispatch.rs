//! `record_event` 顶层入口:按域分派到行为域 / 系统域 writer。

use crate::event::StatsEvent;
use crate::store::StatsStore;

impl StatsStore {
    /// 落一行事件。降级时静默 no-op。`session_id` 由 writer(server)给定。
    ///
    /// # Params:
    ///   - `ts`: 事件时刻 epoch ms
    ///   - `session_id`: 归属会话 id;无会话上下文传 `None`(落 NULL)
    ///   - `event`: 待落库的事件
    pub async fn record_event(
        &self,
        ts: i64,
        session_id: Option<i64>,
        event: &StatsEvent,
    ) -> color_eyre::Result<()> {
        let Some(pool) = self.pool() else {
            return Ok(());
        };
        match event {
            StatsEvent::Behavior { actor, event } => {
                super::behavior::write(pool, ts, session_id, *actor, event).await
            }
            StatsEvent::System(event) => super::system::write(pool, ts, session_id, event).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::event::{BehaviorEvent, StatsEvent, SystemEvent};
    use crate::store::StatsStore;
    use crate::vocab::Actor;

    /// 降级句柄上 record_event 不 panic、返回 Ok(行为域 + 系统域各验一次)。
    #[tokio::test]
    async fn disabled_record_event_is_noop() -> color_eyre::Result<()> {
        let store = StatsStore::disabled();
        let behavior = StatsEvent::Behavior {
            actor: Actor::User,
            event: BehaviorEvent::ConfigOverride {
                path: "ui.window_title".to_owned(),
            },
        };
        store.record_event(1000, None, &behavior).await?;
        store
            .record_event(
                1001,
                Some(7),
                &StatsEvent::System(SystemEvent::ConfigReload),
            )
            .await?;
        Ok(())
    }
}

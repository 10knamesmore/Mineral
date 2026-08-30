//! 会话快照的组装、即时 / 节流落盘与读取。
//!
//! 启动接线只把快照中的播放模式应用到 player;队列、进度与音量仍保留在快照中,
//! 但不自动应用。
//!
//! 本模块实现 `PlayerCore` 的会话方法,并在 crate 内读取 `Inner` 状态。

use std::time::Instant;

use mineral_persist::SessionSnapshot;

use crate::player::PlayerCore;

impl PlayerCore {
    /// 从当前播放上下文组装一份 [`SessionSnapshot`](锁不跨 await,调用方在锁内取完即用)。
    ///
    /// 队列存裸 `SongId` 保序;current 取 `current_song.id`;position / volume 读
    /// audio snapshot(`volume_pct` 0..=100 → `f64` 0.0..=1.0);play_mode 存 Debug 名稳定串。
    ///
    /// # Return:
    ///   组装好的 [`SessionSnapshot`]。
    pub(crate) fn snapshot_session(&self) -> SessionSnapshot {
        let audio = self.inner.audio.snapshot();
        let st = self.inner.state.lock();
        SessionSnapshot {
            current: st.current_song.as_ref().map(|s| s.id.clone()),
            position_ms: audio.position_ms,
            play_mode: st.play_mode.name().to_owned(),
            volume: f64::from(audio.volume_pct) / 100.0,
            queue: st.queue.iter().map(|s| s.id.clone()).collect(),
        }
    }

    /// fire-and-forget 落盘当前会话:snapshot 在 spawn **前**组装好(锁不跨 await),
    /// owned move 进 task;失败仅 warn。降级 persist 下 save 自动 no-op。
    pub(crate) fn spawn_save_session(&self) {
        let snap = self.snapshot_session();
        let persist = self.inner.persist.clone();
        tokio::spawn(async move {
            if let Err(e) = persist.session().save(&snap).await {
                mineral_log::warn!(target: "player", error = mineral_log::chain(&e), "会话保存失败");
            }
        });
    }

    /// 读回上次会话快照;本方法不修改播放状态。
    ///
    /// # Return:
    ///   上次会话;无历史 / 降级 persist 返回 `Ok(None)`。
    pub(crate) async fn load_session(&self) -> color_eyre::Result<Option<SessionSnapshot>> {
        self.inner.persist.session().load().await
    }

    /// 节流落盘:距上次周期 save 超过配置的 `session_save` 间隔才 save 一次(主要刷新 position)。
    /// 状态变化类 save 走各自的即时 [`Self::spawn_save_session`],此处只补周期进度。
    ///
    /// **空态守卫**:无当前曲且队列为空(如 daemon 刚启动还没人播)不落盘——空态没有
    /// 进度可刷,落盘只会用空快照覆盖上次非空会话的队列与进度。
    pub(crate) fn check_session_save(&self) {
        {
            let mut last = self.inner.last_session_save.lock();
            if last.elapsed() < self.inner.session_save {
                return;
            }
            *last = Instant::now();
        }
        {
            let st = self.inner.state.lock();
            if st.current_song.is_none() && st.queue.is_empty() {
                return;
            }
        }
        self.spawn_save_session();
    }
}

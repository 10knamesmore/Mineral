//! 全曲振幅包络的编排:db 命中直推,缺失离线解码 → 落库 → 推送。
//!
//! 不进 task lane——lane 按 channel 路由,包络没有 channel(纯本地解码),
//! 走 `TaskEvent::EnvelopeReady` 直推 client_events(同 `LibrarySnapshot` 先例)。
//! in-flight 守卫防同曲重复解码:开播 / gapless 预排 / 缓存收割多路都可能触发。

use std::path::PathBuf;

use mineral_model::{Envelope, SongId};
use mineral_task::TaskEvent;

use crate::player::PlayerCore;

impl PlayerCore {
    /// 确保一首歌的包络可用并推给 client:db 命中直推;缺失则离线解码、落库后推。
    /// 任何失败只记日志(渲染侧自然回落普通进度条),不冒泡。
    ///
    /// # Params:
    ///   - `song_id`: 目标歌曲
    ///   - `path`: 该曲**可完整读取**的本地文件(缓存 / 下载导出 / 本地曲库;
    ///     流播半截的 capture 解不出全曲形状,不要传进来)
    pub(crate) fn ensure_envelope(&self, song_id: SongId, path: PathBuf) {
        let player = self.clone();
        tokio::spawn(async move {
            match player.cached_envelope(&song_id).await {
                Some(envelope) => player.push_envelope_ready(song_id, envelope),
                None => player.compute_envelope(song_id, path).await,
            }
        });
    }

    /// 对当前在播曲补推一次 db 缓存的包络(gapless adopt 边界 / client 重连重放用),
    /// **不触发计算**——db 缺失时静默无事。
    pub(crate) fn replay_current_envelope(&self) {
        let Some(song_id) = self.with_state(|st| st.current_song.as_ref().map(|s| s.id.clone()))
        else {
            return;
        };
        let player = self.clone();
        tokio::spawn(async move {
            if let Some(envelope) = player.cached_envelope(&song_id).await {
                player.push_envelope_ready(song_id, envelope);
            }
        });
    }

    /// 读 db 缓存的当前版本包络;读失败 warn 后按缺失处理(触发重算兜底)。
    async fn cached_envelope(&self, song_id: &SongId) -> Option<Envelope> {
        let scope = self.persist().scope(song_id.namespace());
        match scope
            .get_envelope(song_id, mineral_audio::ENVELOPE_VERSION)
            .await
        {
            Ok(hit) => hit,
            Err(e) => {
                mineral_log::warn!(target: "player", error = mineral_log::chain(&e), "读包络缓存失败");
                None
            }
        }
    }

    /// 离线解码整曲算包络(in-flight 去重),成功后落库并推送。
    async fn compute_envelope(&self, song_id: SongId, path: PathBuf) {
        if !self
            .inner
            .envelope_inflight
            .lock()
            .insert(song_id.qualified())
        {
            return;
        }
        let computed =
            tokio::task::spawn_blocking(move || mineral_audio::envelope_from_file(&path)).await;
        self.inner
            .envelope_inflight
            .lock()
            .remove(&song_id.qualified());
        let envelope = match computed {
            Ok(Ok(envelope)) => envelope,
            Ok(Err(e)) => {
                mineral_log::warn!(target: "player", song = song_id.as_str(), error = mineral_log::chain(&e), "包络计算失败");
                return;
            }
            Err(e) => {
                mineral_log::warn!(target: "player", error = mineral_log::chain(&e), "包络计算线程 join 失败");
                return;
            }
        };
        let scope = self.persist().scope(song_id.namespace());
        if let Err(e) = scope.put_envelope(&song_id, &envelope).await {
            // 落库失败仍推送:本次会话波形照常,只是重启后要重算。
            mineral_log::warn!(target: "player", error = mineral_log::chain(&e), "包络落库失败");
        }
        self.push_envelope_ready(song_id, envelope);
    }

    /// 推 `EnvelopeReady` 进 client_events buffer(client 侧按归属当前曲装载)。
    fn push_envelope_ready(&self, song_id: SongId, envelope: Envelope) {
        self.inner
            .client_events
            .lock()
            .push(TaskEvent::EnvelopeReady { song_id, envelope });
    }
}

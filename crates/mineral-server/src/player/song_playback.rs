//! 起播、切歌结算与媒体信息富化，统一记录播放语境和收听结果。

use std::sync::atomic::Ordering;

use mineral_model::{Song, SongId};
use mineral_protocol::{PlayCursor, PlaybackOrigin};
use mineral_task::{ChannelFetchKind, Priority, TaskKind};

use super::PlayerCore;

impl PlayerCore {
    /// 建立一首歌的播放状态并启动媒体解析。
    ///
    /// **不结算上一首**:自然 EOF / next / prev / stop 各路径已在调用前按各自语义
    /// (eof / skip / stop)结算;直接改播入口(client `PlaySong` / 脚本 `Play`)须先调
    /// [`Self::settle_interrupted`],否则被打断曲经 recorder 的自愈防线按近似值兜底。
    ///
    /// # Params:
    ///   - `song`: 要播放的歌
    ///   - `play_origin`: 起播来源(埋点 provenance:显式点播 / 自动接续 / 脚本)
    ///   - `actor`: 发起方(用户按键 / 脚本 / daemon 自治;与 origin 独立)
    pub fn play_song(
        &self,
        song: &Song,
        play_origin: mineral_stats::PlayOrigin,
        actor: mineral_stats::Actor,
    ) {
        mineral_log::info!(
            target: "player",
            song_id = song.id.as_str(),
            title = %song.name,
            "play song"
        );
        // 这里只取消旧歌词拉取；播放实例由下方 slot 生命周期负责取消。
        self.inner
            .scheduler
            .cancel_where(|k| matches!(k, TaskKind::ChannelFetch(ChannelFetchKind::Lyrics { .. })));
        self.inner.audio.stop();
        let local_hit = crate::resolve::resolve_local(
            &self.inner.media_cache,
            self.inner.music_dir.as_deref(),
            song,
            self.inner.playback_quality,
        );
        let origin = local_hit
            .as_ref()
            .map_or(PlaybackOrigin::Remote, |hit| hit.origin);
        let slot = crate::playback_instance::PlaybackSlot::new(song.id.clone());

        {
            let mut st = self.inner.state.lock();
            if let Some(current) = st.current_slot.take() {
                current.cancel();
            }
            drop(st.take_prefetch());
            st.current_song = Some(song.clone());
            // 仅当游标尚未指向本曲时才按身份 first-match 定位(列表点歌入口)。
            // 顺序推进入口(advance_next/advance_prev)已把游标钉到精确下标,这里
            // 必须保留——否则队列里有重复曲时,first-match 会把下标拽回首个副本,两首交替
            // 的重复曲互相指回对方、无限循环跳不出去。
            // 本曲在队列里找得到即离开悬空态;找不到(播队列外的歌)则保持原游标不动。
            let already_positioned = st
                .cursor
                .queue_index()
                .and_then(|at| st.queue.get(at))
                .is_some_and(|s| s.id == song.id);
            if !already_positioned && let Some(idx) = st.queue.iter().position(|s| s.id == song.id)
            {
                st.cursor = PlayCursor::InQueue(idx);
            }
            st.current_slot = Some(slot.clone());
            st.media_info = None;
            st.direct_media = None;
            st.play_origin = Some(origin);
            st.current_lyrics = None;
            st.current_lyrics_song_id = None;
            st.bump_current();
        }
        // 埋点:起播语境快照(origin / actor 由调用点穿透;context 经 take_play_context
        // 消费 per-song 覆盖或继承队列级语境;format 等 resolved 快照随后经 enrich 补;
        // 时钟异常拿不出起播时刻则本次不记)。
        let play_mode = self.with_state(|st| st.play_mode);
        let context = self.take_play_context(&song.id);
        if let Some(pending) = crate::pending_from_start(
            song.clone(),
            crate::stats_play_mode(play_mode),
            song.duration_ms.and_then(|d| i64::try_from(d).ok()),
            origin,
            play_origin,
            actor,
            context,
        ) {
            self.inner.stats.play_started(pending);
        }
        // 对齐 finished_seq,防止 audio.stop() 极端时序下被旧 seq 误触发。
        let seq = self.inner.audio.snapshot().track_finished_seq;
        self.inner
            .last_seen_finished_seq
            .store(seq, Ordering::Relaxed);

        crate::playback::start_current(self, song.clone(), slot, local_hit);
        mineral_log::debug!(target: "player", song_id = song.id.as_str(), source = ?song.source(), "submit Lyrics task");
        self.inner.scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::Lyrics {
                song_id: song.id.clone(),
            }),
            Priority::User,
        );
        self.spawn_save_session();
    }

    /// 取该曲的起播语境:优先消费 per-song 覆盖(插队散曲,取后移除),否则继承队列级
    /// 语境。所有起播路径(play_song / gapless adopt)统一走这里,防止某条路径绕过覆盖
    /// 把插队曲记成队列归属。
    ///
    /// # Params:
    ///   - `id`: 起播曲 id
    ///
    /// # Return:
    ///   该曲应记入 plays 的语境
    pub(crate) fn take_play_context(&self, id: &SongId) -> mineral_stats::QueueContext {
        self.with_state(|st| {
            st.context_overrides
                .remove(&id.qualified())
                .unwrap_or_else(|| st.queue_context.clone())
        })
    }

    /// 结算被直接改播打断的在播曲(按 skip):直接改播入口(client `PlaySong` / 脚本
    /// `Play`)在调 [`Self::play_song`] 前用。next / prev / EOF / stop 各有自己的结算,
    /// 不走这里。无在播曲时 no-op。
    ///
    /// 必须在新曲 `play_song` **之前**调用:结算取的是被打断曲的实时播放位置,且
    /// recorder 按 FIFO 先消化本结算再开新 pending。
    pub(crate) fn settle_interrupted(&self) {
        let Some(old) = self.with_state(|st| st.current_song.clone()) else {
            return;
        };
        let position_ms = self.inner.audio.snapshot().position_ms;
        self.spawn_on_played(
            old.id.clone(),
            mineral_stats::FinishReason::Skip,
            position_ms,
        );
        self.inner
            .notify
            .track_finished(&old, mineral_protocol::FinishReason::Skip);
    }

    /// 以已生效媒体事实富化在播行的音频列。
    ///
    /// Current 与 gapless promotion 都必须调用本入口，使 hook 顶换标记进入同一 stats snapshot。
    ///
    /// # Params:
    ///   - `info`: 已生效契约的展示元信息
    pub(crate) fn enrich_from_media_info(&self, info: &mineral_model::PlaybackMediaInfo) {
        self.inner
            .stats
            .enrich_play_audio(mineral_stats::PlayAudioSnapshot {
                audio_format: info.format.clone(),
                bitrate_bps: info.bitrate_bps.map(i64::from),
                quality: Some(info.quality),
                bit_depth: info.bit_depth.map(i64::from),
                substituted: info.substituted,
            });
    }

    /// 异步上报一次播放打点(fire-and-forget,不阻塞播放)。
    ///
    /// # Params:
    ///   - `id`: 歌曲
    ///   - `reason`: 结束原因(自然播完 eof / 被切 skip;stop / error 走各自站点)
    ///   - `listen_ms`: 本次收听毫秒
    pub(crate) fn spawn_on_played(
        &self,
        id: SongId,
        reason: mineral_stats::FinishReason,
        listen_ms: u64,
    ) {
        // 埋点:结算在播行(起播被 gate 掉时 actor 无 pending、自动忽略)。
        self.inner
            .stats
            .play_ended(reason, i64::try_from(listen_ms).unwrap_or(i64::MAX));
        let Some(channel) = self.channel_for(id.namespace()) else {
            return;
        };
        let channel = channel.clone();
        // channel 契约的上报语义是完成布尔:eof 视为完整播完,其余为中断。
        let completed = matches!(reason, mineral_stats::FinishReason::Eof);
        tokio::spawn(async move {
            if let Err(e) = channel.on_played(&id, completed, listen_ms).await {
                mineral_log::warn!(target: "player", error = mineral_log::chain(&e), "on_played 打点失败");
            }
        });
    }
}

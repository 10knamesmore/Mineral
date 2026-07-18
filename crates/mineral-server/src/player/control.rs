//! 队列编排与播放控制:替换 / 插入 / 追加队列、播放模式切换、上 / 下一首。

use mineral_channel_core::ChannelCaps;
use mineral_model::{Song, SongId, SourceKind};
use mineral_protocol::PlayMode;
use rand::seq::SliceRandom;

use super::PlayerCore;
use crate::queue::{advance_next, advance_prev, apply_play_mode};

impl PlayerCore {
    /// 替换 queue。等价历史 `App::set_queue`。
    ///
    /// # Params:
    ///   - `new_queue`: 新队列
    ///   - `target_id`: 队列中作为「当前」的歌
    ///   - `context`: 队列语境(埋点 provenance:随该队列每个起播继承)
    pub fn set_queue(
        &self,
        mut new_queue: Vec<Song>,
        target_id: &SongId,
        context: mineral_stats::QueueContext,
    ) {
        // 队列硬上限:超长替换截断到 QUEUE_CAP(下标 0-based 故最大 9998,与序号显示上限一致)。
        new_queue.truncate(crate::queue::QUEUE_CAP);
        {
            let mut st = self.inner.state.lock();
            mineral_log::info!(
                target: "player",
                len = new_queue.len(),
                target_id = target_id.as_str(),
                mode = ?st.play_mode,
                "set queue"
            );
            st.queue_context = context;
            // 换队列 = 旧队列的 per-song 语境覆盖全部作废:未播插队曲的条目不清会永久
            // 残留(泄漏),且同 id 歌在新队列起播时会误继承陈旧语境、污染归属统计。
            st.context_overrides.clear();
            if matches!(st.play_mode, PlayMode::Shuffle) {
                let mut shuffled = new_queue.clone();
                shuffled.shuffle(&mut rand::rng());
                if let Some(pos) = shuffled.iter().position(|s| s.id == *target_id) {
                    shuffled.swap(0, pos);
                }
                st.original_queue = Some(new_queue);
                st.queue = shuffled;
                st.queue_sel = 0;
            } else {
                let sel = new_queue
                    .iter()
                    .position(|s| s.id == *target_id)
                    .unwrap_or(0);
                st.queue = new_queue;
                st.queue_sel = sel;
                st.original_queue = None;
            }
            // 换队列后已预排的下一曲可能不再是新队列的 next:作废,让 check_prefetch 按新队列重排。
            st.invalidate_prefetch();
            st.bump_queue();
        }
        // 取消引擎里尚未 append 的待建预排(已 append 的无法摘除,会自然播完后由边界兜底)。
        self.inner.audio.clear_next();
        self.spawn_save_session();
    }

    /// 插播:插到当前曲之后,不动队列级 context 与当前曲。
    ///
    /// # Params:
    ///   - `song`: 待插播的歌
    ///   - `context`: 该曲来源语境(落 per-song 覆盖:起播时用它而非队列级 context)
    pub fn queue_insert_next(&self, song: Song, context: mineral_stats::QueueContext) {
        {
            let mut st = self.inner.state.lock();
            st.context_overrides.insert(song.id.qualified(), context);
            crate::queue::insert_next(&mut st, song);
            // 下一首变了:作废已排的 gapless 预排,让 check_prefetch 重排
            st.invalidate_prefetch();
        }
        self.inner.audio.clear_next();
        self.spawn_save_session();
    }

    /// 追加到队列末尾,不动队列级 context 与当前曲。
    /// 当前曲恰在尾部时"下一首"会变,保守作废预排(与插播同样处理)。
    ///
    /// # Params:
    ///   - `song`: 待追加的歌
    ///   - `context`: 该曲来源语境(落 per-song 覆盖:同插播)
    pub fn queue_append(&self, song: Song, context: mineral_stats::QueueContext) {
        {
            let mut st = self.inner.state.lock();
            st.context_overrides.insert(song.id.qualified(), context);
            crate::queue::append(&mut st, song);
            st.invalidate_prefetch();
        }
        self.inner.audio.clear_next();
        self.spawn_save_session();
    }

    /// 全部已注册 channel 的能力声明(按注册顺序)。
    pub fn channel_caps(&self) -> Vec<(SourceKind, ChannelCaps)> {
        self.inner
            .channels
            .iter()
            .map(|ch| (ch.source(), ch.caps()))
            .collect()
    }

    /// `m` 键:PlayMode cycle + 进/退 Shuffle 边界处洗牌或还原(+ 记 mode_changes)。
    ///
    /// # Params:
    ///   - `actor`: 发起方
    pub fn cycle_play_mode(&self, actor: mineral_stats::Actor) {
        let (from, to) = {
            let mut st = self.inner.state.lock();
            let from = st.play_mode;
            let new = st.play_mode.cycle();
            apply_play_mode(&mut st, new);
            (from, st.play_mode)
        };
        self.record_mode_change(actor, from, to);
        self.spawn_save_session();
    }

    /// 直接设目标 PlayMode(系统媒体控件按维度写 Shuffle/LoopStatus 后塌缩成的档,
    /// 脚本 set_mode 同入口;+ 记 mode_changes,设成同值不记)。
    ///
    /// # Params:
    ///   - `mode`: 目标模式
    ///   - `actor`: 发起方
    pub fn set_play_mode(&self, mode: PlayMode, actor: mineral_stats::Actor) {
        let (from, to) = {
            let mut st = self.inner.state.lock();
            let from = st.play_mode;
            apply_play_mode(&mut st, mode);
            (from, st.play_mode)
        };
        self.record_mode_change(actor, from, to);
        self.spawn_save_session();
    }

    /// 模式切换的埋点出口(cycle / 直设共用;无变化不记)。
    fn record_mode_change(&self, actor: mineral_stats::Actor, from: PlayMode, to: PlayMode) {
        if from == to {
            return;
        }
        self.record_behavior(
            actor,
            mineral_stats::BehaviorEvent::ModeChange {
                from_mode: crate::stats_play_mode(from),
                to_mode: crate::stats_play_mode(to),
            },
        );
    }

    /// 启动时恢复上次会话的播放模式:只写模式标志,不走 [`Self::set_play_mode`] 的
    /// 洗牌/还原边界(此刻队列为空,无可洗),也不回写会话(快照其余字段原样)。
    ///
    /// # Params:
    ///   - `mode`: 上次会话解析出的播放模式
    pub fn restore_play_mode(&self, mode: PlayMode) {
        let mut st = self.inner.state.lock();
        st.play_mode = mode;
    }

    /// `p` 键:进度 > 阈值 → seek(0);否则跳上一首。
    ///
    /// # Params:
    ///   - `actor`: 发起方(用户按键 / 脚本 / 媒体键;origin 恒 AutoAdvance,两维独立)
    pub fn prev_or_restart(&self, actor: mineral_stats::Actor) {
        let pos = self.inner.audio.snapshot().position_ms;
        let (old, prev) = {
            let mut st = self.inner.state.lock();
            if st.current_song.is_none() {
                return;
            }
            if pos > self.inner.prev_restart_threshold_ms {
                drop(st);
                // 回开头不算切歌/跳过,不打点。
                self.inner.audio.seek(0);
                return;
            }
            // advance_prev 把 queue_sel 钉到上一首下标,play_song 据守卫保留它(见其内注释)。
            (st.current_song.clone(), advance_prev(&mut st))
        };
        if let Some(s) = prev {
            if let Some(old) = old {
                self.spawn_on_played(old.id.clone(), mineral_stats::FinishReason::Skip, pos);
                self.inner
                    .notify
                    .track_finished(&old, mineral_protocol::FinishReason::Skip);
            }
            self.play_song(&s, mineral_stats::PlayOrigin::AutoAdvance, actor);
        }
    }

    /// `n` 键:按 PlayMode 切下一首。
    ///
    /// # Params:
    ///   - `actor`: 发起方(用户按键 / 脚本 / 媒体键;origin 恒 AutoAdvance,两维独立)
    pub fn next_song(&self, actor: mineral_stats::Actor) {
        let position_ms = self.inner.audio.snapshot().position_ms;
        let (old, next) = {
            let mut st = self.inner.state.lock();
            // advance_next 把 queue_sel 钉到下一首下标,play_song 据守卫保留它(见其内注释)。
            (st.current_song.clone(), advance_next(&mut st))
        };
        if let Some(s) = next {
            if let Some(old) = old {
                self.spawn_on_played(
                    old.id.clone(),
                    mineral_stats::FinishReason::Skip,
                    position_ms,
                );
                self.inner
                    .notify
                    .track_finished(&old, mineral_protocol::FinishReason::Skip);
            }
            self.play_song(&s, mineral_stats::PlayOrigin::AutoAdvance, actor);
        }
    }
}

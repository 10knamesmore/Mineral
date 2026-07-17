//! 播放传输面(暂停 / 恢复 / 跳转 / 音量)——音频直通与埋点采集的统一出口。
//!
//! client Handler、脚本命令、系统媒体控件三条入口都必须走这里,不许绕过它直调
//! `audio()`:采集与执行同点,新入口才不可能「操作生效但埋点漏记」(actor 由入口
//! 穿透,脚本 / 媒体键的操作与界面按键同样入库)。

use mineral_model::SongId;
use mineral_stats::{Actor, BehaviorEvent, PauseAction};

use super::PlayerCore;

impl PlayerCore {
    /// 记一次行为域事件(stamp now_ms)。
    ///
    /// # Params:
    ///   - `actor`: 发起方
    ///   - `event`: 行为事件本体
    pub(crate) fn record_behavior(&self, actor: Actor, event: BehaviorEvent) {
        self.inner
            .stats
            .event(mineral_stats::StatsEvent::Behavior { actor, event });
    }

    /// 当前在播曲 id + 播放位置 ms(pause / seek 事件富化);无在播曲返回 `None`。
    fn current_play(&self) -> Option<(SongId, i64)> {
        let song = self.with_state(|st| st.current_song.clone())?;
        let position = i64::try_from(self.inner.audio.snapshot().position_ms).unwrap_or(i64::MAX);
        Some((song.id, position))
    }

    /// 暂停(+ 记 pauses)。
    ///
    /// # Params:
    ///   - `actor`: 发起方
    pub(crate) fn pause_playback(&self, actor: Actor) {
        self.inner.audio.pause();
        if let Some((song, at_ms)) = self.current_play() {
            self.record_behavior(
                actor,
                BehaviorEvent::Pause {
                    song,
                    at_ms,
                    action: PauseAction::Pause,
                },
            );
        }
    }

    /// 恢复(+ 记 pauses)。
    ///
    /// # Params:
    ///   - `actor`: 发起方
    pub(crate) fn resume_playback(&self, actor: Actor) {
        self.inner.audio.resume();
        if let Some((song, at_ms)) = self.current_play() {
            self.record_behavior(
                actor,
                BehaviorEvent::Pause {
                    song,
                    at_ms,
                    action: PauseAction::Resume,
                },
            );
        }
    }

    /// 暂停 / 恢复翻转(脚本 toggle 与媒体键 toggle 用)。
    ///
    /// # Params:
    ///   - `actor`: 发起方
    pub(crate) fn toggle_playback(&self, actor: Actor) {
        if self.inner.audio.snapshot().playing {
            self.pause_playback(actor);
        } else {
            self.resume_playback(actor);
        }
    }

    /// 跳转(+ 记 seeks;from 取跳转前的实时位置,须在下发 seek 之前抓)。
    ///
    /// # Params:
    ///   - `position_ms`: 目标位置 ms
    ///   - `actor`: 发起方
    pub(crate) fn seek_playback(&self, position_ms: u64, actor: Actor) {
        let before = self.current_play();
        self.inner.audio.seek(position_ms);
        if let Some((song, from_ms)) = before {
            self.record_behavior(
                actor,
                BehaviorEvent::Seek {
                    song,
                    from_ms,
                    to_ms: i64::try_from(position_ms).unwrap_or(i64::MAX),
                },
            );
        }
    }

    /// 设音量(+ 记 volume_changes;from 取设前音量)。
    ///
    /// # Params:
    ///   - `pct`: 目标音量百分比
    ///   - `actor`: 发起方
    pub(crate) fn set_playback_volume(&self, pct: u8, actor: Actor) {
        let from = self.inner.audio.snapshot().volume_pct;
        self.inner.audio.set_volume(pct);
        self.record_behavior(
            actor,
            BehaviorEvent::VolumeChange {
                from_pct: i64::from(from),
                to_pct: i64::from(pct),
            },
        );
    }
}

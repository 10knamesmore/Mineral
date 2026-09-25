//! 从 client 订阅镜像同步 App 状态,并把操作完成事件收敛成 UI 反馈。
//!
//! 每帧一次纯本地内存读,不产生 IPC;版本一致时连队列重段都不构造(直接跳过),
//! UI 自有状态不被推送覆盖。

use std::time::Instant;

use mineral_client::operation::Outcome;
use mineral_client::state::WindowTitleOverride;
use mineral_protocol::{PlayerSync, QueueEditOutcome, QueueSync, SubscriptionTopic};

use crate::components::toast::notifications::{TextTint, tinted_text_item};
use crate::runtime::backend::Completion;

use super::App;

impl App {
    /// 把镜像当前状态灌进 App 状态(每帧调一次,纯本地读)。
    pub(super) fn sync_from_backend(&mut self) {
        // 播放锚点:位置用本地单调钟推进,其余字段照抄权威值。
        let mut anchor = mineral_audio::AudioSnapshot::default();
        let mut position = 0_u64;
        self.client.with_playback(&mut |playback| {
            anchor = *playback.anchor();
            position = playback.position_ms(Instant::now());
        });
        anchor.position_ms = position;
        self.state.playback.apply_audio_snapshot(anchor);

        // 模式、光标和来源每帧读取;队列与当前曲重段按版本复制。
        let known = self.state.player.versions;
        let mut sync = None;
        self.client.with_player(&mut |player| {
            self.state.playback.mode = player.play_mode();
            self.state.playback.play_origin = player.play_origin();
            self.state.player.cursor = player.cursor();

            let versions = player.versions();
            if versions != known {
                sync = Some(PlayerSync {
                    versions,
                    cursor: player.cursor(),
                    play_mode: player.play_mode(),
                    play_origin: player.play_origin(),
                    queue: (known.queue != versions.queue).then(|| QueueSync {
                        queue: player.queue().to_vec(),
                        original_queue: player
                            .original_queue()
                            .map(<[mineral_model::Song]>::to_vec),
                    }),
                    current: (known.current != versions.current)
                        .then(|| player.current().cloned())
                        .flatten(),
                });
            }
        });
        if let Some(sync) = sync {
            self.apply_player_sync(sync);
        }
        self.state
            .transport
            .sync_mode(self.state.playback.mode, self.state.cfg.tui().animation());

        if let Some(tasks) = self.client.tasks() {
            self.state.tasks_snapshot = tasks;
        }
        let summary = self.client.downloads_summary();
        self.download_notifier
            .feed(&mut self.notifications, &summary);
        self.state.downloads_summary = summary;
        self.sync_downloads_subscription();

        if self.state.window_title_override.is_none()
            && let WindowTitleOverride::Set(text) = self.client.window_title_override()
        {
            self.state.window_title_override = text;
        }
    }

    /// Downloads 浮层生命周期 ↔ 明细订阅生命周期。
    fn sync_downloads_subscription(&mut self) {
        let open = self.overlays.has_downloads();
        if open == self.downloads_subscribed {
            if open && let Some(detail) = self.client.downloads_detail() {
                self.state.downloads = detail.rows();
                self.overlays.clamp_downloads(self.state.downloads.len());
            }
            return;
        }
        self.downloads_subscribed = open;
        if open {
            self.client.subscribe(SubscriptionTopic::DownloadsDetail);
        } else {
            self.client.unsubscribe(SubscriptionTopic::DownloadsDetail);
            self.state.downloads.clear();
        }
    }

    /// 取走操作完成事件并给出 UI 反馈。
    pub(super) fn drain_completions(&mut self) {
        for completion in self.completions.drain() {
            self.apply_completion(completion);
        }
    }

    /// 应用一条完成事件。
    fn apply_completion(&mut self, completion: Completion) {
        match completion {
            Completion::PlayQueue(outcome) => match outcome {
                // Applied 只承载成功;业务失败在 client 侧已归一 `Failed`(见
                // `Client::play_queue` 的载荷译码)。
                Outcome::Failed { detail, .. } | Outcome::Unknown { detail } => {
                    self.notifications
                        .flash(tinted_text_item(detail, TextTint::Error));
                }
                Outcome::Applied(_) | Outcome::Accepted(_) => {}
            },
            Completion::QueueEdit(outcome) => match outcome {
                Outcome::Applied(QueueEditOutcome::Stale)
                | Outcome::Accepted(QueueEditOutcome::Stale) => {
                    self.notifications.flash(tinted_text_item(
                        "queue changed elsewhere, nothing done".to_owned(),
                        TextTint::Error,
                    ));
                }
                Outcome::Failed { detail, .. } | Outcome::Unknown { detail } => {
                    self.notifications
                        .flash(tinted_text_item(detail, TextTint::Error));
                }
                Outcome::Applied(_) | Outcome::Accepted(_) => {}
            },
            Completion::ScriptAction { name, outcome } => {
                if let Some(message) = completion_failure(&outcome) {
                    self.notifications.flash(tinted_text_item(
                        format!("{name}: {message}"),
                        TextTint::Error,
                    ));
                }
            }
            Completion::CopyTemplate(outcome) => match outcome {
                Outcome::Applied(Ok(text)) => self.copy_to_clipboard(&text),
                Outcome::Applied(Err(message)) => {
                    self.notifications
                        .flash(tinted_text_item(message, TextTint::Error));
                }
                Outcome::Failed { detail, .. } | Outcome::Unknown { detail } => {
                    self.notifications
                        .flash(tinted_text_item(detail, TextTint::Error));
                }
                Outcome::Accepted(_) => {
                    self.notifications
                        .flash(tinted_text_item("复制失败".to_owned(), TextTint::Error));
                }
            },
            Completion::Love { song_id, outcome } => {
                // 服务端结论为准:失败提示;成功时订阅刷新会校正乐观值。
                if let Outcome::Failed { detail, .. } | Outcome::Unknown { detail } = outcome {
                    mineral_log::warn!(
                        target: "tui",
                        song_id = song_id.as_str(),
                        detail,
                        "喜欢切换未成功"
                    );
                    self.notifications
                        .flash(tinted_text_item(detail, TextTint::Error));
                }
            }
            Completion::StopDownload(outcome) => {
                if let Some(message) = completion_failure(&outcome) {
                    self.notifications
                        .flash(tinted_text_item(message, TextTint::Error));
                }
            }
            Completion::ScriptBinds(binds) => self.apply_script_binds(&binds),
        }
    }
}

/// 完成结论 → 失败文案(`Applied` / `Accepted` 为 `None`)。
fn completion_failure<T>(outcome: &Outcome<T>) -> Option<String> {
    match outcome {
        Outcome::Failed { detail, .. } => Some(detail.clone()),
        Outcome::Unknown { detail } => Some(detail.clone()),
        Outcome::Applied(_) | Outcome::Accepted(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{PlayCursor, PlayMode, PlaybackOrigin};

    use crate::test_support::app_with_queue;

    /// 重段版本相同时仍从后端镜像刷新轻量状态,并保留已有队列与当前曲。
    #[test]
    fn backend_sync_refreshes_light_state_without_section_changes() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        let queue_before = app.state.player.queue.clone();
        let current_before = app.state.player.current.clone();
        let versions_before = app.state.player.versions;
        app.state.playback.mode = PlayMode::RepeatOne;
        app.state.playback.play_origin = Some(PlaybackOrigin::Remote);
        app.state.player.cursor = PlayCursor::InQueue(2);

        // 测试后端的镜像是顺序播放,与 UI 持有相同的重段版本。
        app.client.with_player(&mut |player| {
            assert_eq!(player.versions(), versions_before);
            assert_eq!(player.play_mode(), PlayMode::Sequential);
        });
        app.sync_from_backend();

        assert_eq!(app.state.playback.mode, PlayMode::Sequential);
        assert_eq!(app.state.playback.play_origin, None);
        assert_eq!(app.state.player.cursor, PlayCursor::InQueue(0));
        assert_eq!(app.state.player.versions, versions_before);
        assert_eq!(app.state.player.queue, queue_before);
        assert_eq!(app.state.player.current, current_before);
        assert_eq!(app.state.playback.track, current_before);
        Ok(())
    }
}

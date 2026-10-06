//! 事件通知出口:event hub 给订阅 client 下发 wire 状态与生命周期事件。
//!
//! 生命周期事件(曲终 / 下载完成)与属性变更都从这里出去 —— 业务代码只调
//! [`Notifier`] 的具名方法,不直接摸 broadcast / channel。

use mineral_model::Song;
use mineral_protocol::{Event, FinishReason, PropName, PropValue, TextSpan};
use mineral_script::ScriptSender;
use tokio::sync::broadcast;

use crate::player::PlayerCore;

/// Failure of a script command issued through the server.
#[derive(Debug, thiserror::Error)]
pub(crate) enum ScriptError {
    /// No script is attached to the player.
    #[error("script is disabled")]
    Disabled,

    /// The script callback failed; retain its original cause.
    #[error("script callback failed")]
    Callback(#[source] mineral_script::Error),

    /// The script thread exited before replying.
    #[error("script thread exited")]
    ThreadExited,
}

/// Wire 事件出口与 daemon 音乐回调句柄。无订阅者时事件 send 失败即丢(advisory)。
#[derive(Clone)]
pub(crate) struct Notifier {
    /// wire 路:event hub 发送端,serve 层按握手订阅集过滤下发。
    events: broadcast::Sender<Event>,

    /// Daemon 音乐查询与 hook 的投递句柄;未启用脚本为 `None`。
    script: Option<ScriptSender>,
}

impl Notifier {
    /// 构造 wire 出口并保留音乐回调句柄。
    ///
    /// # Params:
    ///   - `events`: event hub 发送端
    ///   - `script`: 脚本投递句柄(无用户脚本传 `None`)
    pub(crate) fn new(events: broadcast::Sender<Event>, script: Option<ScriptSender>) -> Self {
        Self { events, script }
    }

    /// Returns the attached script sender for constructing side-system hook gates.
    pub(crate) fn script_sender(&self) -> Option<ScriptSender> {
        self.script.clone()
    }

    /// 一首歌结束:向订阅 client 下发 `TrackFinished`。
    ///
    /// # Params:
    ///   - `song`: 结束的歌
    ///   - `reason`: 结束原因
    pub(crate) fn track_finished(&self, song: &Song, reason: FinishReason) {
        let _ = self.events.send(Event::TrackFinished {
            song_id: song.id.clone(),
            reason,
        });
    }

    /// 一首歌下载完成(永久导出落盘;已存在跳过不调用)。
    ///
    /// # Params:
    ///   - `song`: 下载完成的歌
    pub(crate) fn download_completed(&self, song: &Song) {
        let _ = self.events.send(Event::DownloadCompleted {
            song_id: song.id.clone(),
        });
    }

    /// 订阅 wire 路 event hub(测试直收推送断言用;订阅须先于触发动作,
    /// broadcast 不补发历史)。
    #[cfg(test)]
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// 任务 / 数据事件(库快照、收藏集、搜索结果、下钻详情等)→ wire 订阅推送。
    /// 脚本路不投递:任务结果无脚本 hook 面,脚本要数据走 query 停靠。
    ///
    /// # Params:
    ///   - `ev`: 任务事件载荷
    pub(crate) fn task_event(&self, ev: mineral_task::TaskEvent) {
        let _ = self.events.send(Event::Task(Box::new(ev)));
    }

    /// 发送后台失败类别，由 client 根据结构化状态生成提示。
    pub(crate) fn failure(&self, failure: mineral_protocol::FailureNotice) {
        let _ = self.events.send(Event::Failure(failure));
    }

    /// 推送后台或音乐 hook 的诊断提示文本。
    ///
    /// # Params:
    ///   - `kind`: 视觉级别
    ///   - `content`: 单行人读文本
    pub(crate) fn toast(&self, kind: mineral_protocol::ToastKind, content: String) {
        let _ = self.events.send(Event::Toast {
            kind,
            content: vec![TextSpan::plain(content)],
            id: None,
            ttl_secs: None,
        });
    }

    /// per-song 持久 KV 某键变更:wire `StoreChanged`(粗粒度,只报歌 + 键)。
    ///
    /// # Params:
    ///   - `song_id`: 变更的歌
    ///   - `key`: 变更的键
    pub(crate) fn store_changed(&self, song_id: &mineral_model::SongId, key: &str) {
        let _ = self.events.send(Event::StoreChanged {
            song_id: song_id.clone(),
            key: key.to_owned(),
        });
    }

    /// 广播 daemon 当前可用的队列操作与统计能力，不包含私有配置。
    pub(crate) fn service_info_changed(&self, info: mineral_protocol::ServiceInfo) {
        let _ = self.events.send(Event::ServiceInfoChanged { info });
    }

    /// 属性树某项变更:向订阅 client 下发 `PropertyChanged`。
    ///
    /// # Params:
    ///   - `prop`: 协议属性键
    ///   - `value`: 新值
    pub(crate) fn property_changed(&self, prop: PropName, value: &PropValue) {
        let _ = self.events.send(Event::PropertyChanged {
            prop,
            value: value.clone(),
        });
    }
}

impl PlayerCore {
    /// 停止播放并通知(`Stop` 是 best-effort:当前曲存在才发,不保证与
    /// audio 实际停止严格同步)。client 的 `stop` 与 MPRIS 的 Stop 都走这里。
    pub(crate) fn stop_playback(&self) {
        let current = self.with_state(|st| st.current_song.clone());
        let position = self.inner.audio.snapshot().position_ms;
        self.inner.audio.stop();
        if let Some(song) = current {
            self.inner.notify.track_finished(&song, FinishReason::Stop);
            // 埋点:结算被停的在播行(stop 不走 spawn_on_played,单独结算防 pending 被下次
            // 起播覆盖丢失)。actor 无 pending 时自动忽略。
            let listen = i64::try_from(position).unwrap_or(i64::MAX);
            self.inner
                .stats
                .play_ended(mineral_stats::FinishReason::Stop, listen);
        }
    }

    /// 事件通知出口(crate 内部:gapless 推进 / 下载 worker 用)。
    pub(crate) fn notify(&self) -> &Notifier {
        &self.inner.notify
    }

    /// 脚本投递句柄(查询泵回投结果用);未启用脚本为 `None`。
    pub(crate) fn script_sender(&self) -> Option<mineral_script::ScriptSender> {
        self.inner.notify.script.clone()
    }

    /// 跑一个具名队列变换,拿回新的队列顺序(脚本未启用 / 线程已退出即错误)。
    ///
    /// # Params:
    ///   - `name`: daemon 注册的稳定变换名称
    ///   - `queue`: 当前队列(有序)
    ///   - `current`: 在播条目下标(0-based)
    ///   - `selected`: 光标下标(0-based),无则 `None`
    ///
    /// # Return:
    ///   `Ok(ids)` = 新顺序;`Err` = 脚本不可用或回调失败。
    pub(crate) async fn queue_transform(
        &self,
        name: String,
        queue: Vec<mineral_model::Song>,
        current: usize,
        selected: Option<usize>,
    ) -> Result<Vec<mineral_model::SongId>, ScriptError> {
        let Some(script) = &self.inner.notify.script else {
            return Err(ScriptError::Disabled);
        };
        if !script.is_attached() {
            return Err(ScriptError::Disabled);
        }
        script
            .queue_transform(name, queue, current, selected)
            .await
            .map_err(|_recv| ScriptError::ThreadExited)?
            .map_err(ScriptError::Callback)
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{Event, FinishReason, PropName, PropValue, TextSpan};
    use mineral_test::song;

    use super::Notifier;

    #[test]
    fn wire_lane_carries_events_in_order() -> color_eyre::Result<()> {
        use mineral_protocol::ToastKind;
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(/*capacity*/ 8);
        let notifier = Notifier::new(events_tx, /*script*/ None);
        let s = song("1");
        notifier.track_finished(&s, FinishReason::Eof);
        notifier.property_changed(PropName::PLAYER_VOLUME, &PropValue::Int(42));
        notifier.toast(ToastKind::Warn, "下载不可用".to_owned());
        assert_eq!(
            events_rx.try_recv()?,
            Event::TrackFinished {
                song_id: s.id.clone(),
                reason: FinishReason::Eof,
            }
        );
        assert_eq!(
            events_rx.try_recv()?,
            Event::PropertyChanged {
                prop: mineral_protocol::PropName::PLAYER_VOLUME,
                value: mineral_protocol::PropValue::Int(42),
            }
        );
        assert_eq!(
            events_rx.try_recv()?,
            Event::Toast {
                kind: ToastKind::Warn,
                content: vec![TextSpan::plain("下载不可用")],
                id: None,
                ttl_secs: None,
            }
        );
        Ok(())
    }
}

//! 事件通知出口:wire(event hub 给订阅 client)与脚本线程双路投递。
//!
//! 生命周期事件(曲终 / 下载完成)与属性变更都从这里出去 —— 业务代码只调
//! [`Notifier`] 的具名方法,不直接摸 broadcast / channel。

use mineral_model::Song;
use mineral_protocol::{Event, FinishReason};
use mineral_script::{PropKey, PropValue, ScriptEvent, ScriptSender, TrackFinishedReason};
use tokio::sync::broadcast;

use crate::player::PlayerCore;

/// 双路事件出口。wire 路无订阅者 send 失败即丢(advisory);脚本路未启用
/// (无用户脚本)为 `None`,fire-and-forget。
pub(crate) struct Notifier {
    /// wire 路:event hub 发送端,serve 层按握手订阅集过滤下发。
    events: broadcast::Sender<Event>,

    /// 脚本路:daemon → 脚本线程的投递句柄;未启用脚本为 `None`。
    script: Option<ScriptSender>,
}

impl Notifier {
    /// 构造双路出口。
    ///
    /// # Params:
    ///   - `events`: event hub 发送端
    ///   - `script`: 脚本投递句柄(无用户脚本传 `None`)
    pub(crate) fn new(events: broadcast::Sender<Event>, script: Option<ScriptSender>) -> Self {
        Self { events, script }
    }

    /// 一首歌结束:wire `TrackFinished` + 脚本 `track_finished`。
    ///
    /// # Params:
    ///   - `song`: 结束的歌(脚本路携带整首做投影)
    ///   - `reason`: 结束原因(wire 形;脚本形由 [`to_script_reason`] 映射)
    pub(crate) fn track_finished(&self, song: &Song, reason: FinishReason) {
        let _ = self.events.send(Event::TrackFinished {
            song_id: song.id.clone(),
            reason,
        });
        if let Some(script) = &self.script {
            script.send(ScriptEvent::TrackFinished {
                song: Box::new(song.clone()),
                reason: to_script_reason(reason),
            });
        }
    }

    /// 一首歌下载完成(永久导出落盘;已存在跳过不调用)。
    ///
    /// # Params:
    ///   - `song`: 下载完成的歌
    ///   - `path`: 落盘路径
    pub(crate) fn download_completed(&self, song: &Song, path: &std::path::Path) {
        let _ = self.events.send(Event::DownloadCompleted {
            song_id: song.id.clone(),
        });
        if let Some(script) = &self.script {
            script.send(ScriptEvent::DownloadCompleted {
                song: Box::new(song.clone()),
                path: path.to_path_buf(),
            });
        }
    }

    /// 推一条匿名 toast 给订阅 client(下载不可用 / 失败等 daemon 侧提示)。
    ///
    /// # Params:
    ///   - `kind`: 视觉级别
    ///   - `content`: 单行人读文本
    pub(crate) fn toast(&self, kind: mineral_protocol::ToastKind, content: String) {
        let _ = self.events.send(Event::Toast {
            kind,
            content,
            id: None,
            ttl_secs: None,
        });
    }

    /// per-song 持久 KV 某键变更:wire `StoreChanged`(粗粒度,只报歌 + 键)。
    ///
    /// 脚本路不投递 —— store 写本就发自脚本 / client,变更方已知值;
    /// 真有跨方观察需求时再议(避免脚本自己写自己收的回声)。
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

    /// 属性树某项变更:wire `PropertyChanged` + 脚本 `PropertyChanged`。
    ///
    /// # Params:
    ///   - `key`: 属性键(内部形;wire 形按名映射)
    ///   - `value`: 新值
    pub(crate) fn property_changed(&self, key: PropKey, value: &PropValue) {
        let _ = self.events.send(Event::PropertyChanged {
            prop: mineral_protocol::PropName::from_name(key.as_str()),
            value: to_wire_value(value),
        });
        if let Some(script) = &self.script {
            script.send(ScriptEvent::PropertyChanged {
                key,
                value: value.clone(),
            });
        }
    }
}

/// wire 结束原因 → 脚本内部形(同构映射)。
fn to_script_reason(reason: FinishReason) -> TrackFinishedReason {
    match reason {
        FinishReason::Eof => TrackFinishedReason::Eof,
        FinishReason::Skip => TrackFinishedReason::Skip,
        FinishReason::Error => TrackFinishedReason::Error,
        FinishReason::Stop => TrackFinishedReason::Stop,
    }
}

/// 脚本内部属性值 → wire 形(同构映射;脚本侧无 Bool,六属性也不产 Bool)。
fn to_wire_value(value: &PropValue) -> mineral_protocol::PropValue {
    match value {
        PropValue::Int(n) => mineral_protocol::PropValue::Int(*n),
        PropValue::Str(s) => mineral_protocol::PropValue::Str(s.clone()),
        PropValue::None => mineral_protocol::PropValue::None,
    }
}

impl PlayerCore {
    /// 停止播放并通知(`Stop` 是 best-effort:当前曲存在才发,不保证与
    /// audio 实际停止严格同步)。client 的 `stop` 与 MPRIS 的 Stop 都走这里。
    pub(crate) fn stop_playback(&self) {
        let current = self.with_state(|st| st.current_song.clone());
        self.inner.audio.stop();
        if let Some(song) = current {
            self.inner.notify.track_finished(&song, FinishReason::Stop);
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

    /// 触发脚本具名动作并等待结果(daemon 处理 `Request::InvokeAction` 用)。
    ///
    /// # Params:
    ///   - `name`: 动作注册名
    ///   - `ctx`: 按键瞬间的 client 上下文(无界面触发面为 `None`)
    ///
    /// # Return:
    ///   回调执行完成为 `Ok`;脚本未启用 / 未注册 / 执行失败为 `Err`(人读信息)。
    pub(crate) async fn invoke_script_action(
        &self,
        name: &str,
        ctx: Option<mineral_protocol::KeyContext>,
    ) -> color_eyre::Result<()> {
        use color_eyre::eyre::bail;
        let Some(script) = &self.inner.notify.script else {
            bail!("脚本未启用(无 config.lua 或脚本加载失败)");
        };
        match script.invoke_action(name.to_owned(), ctx).await {
            Ok(mineral_script::ActionOutcome::Done) => Ok(()),
            Ok(mineral_script::ActionOutcome::NotFound) => {
                bail!("动作 {name:?} 未注册(检查 config.lua 的 mineral.action)")
            }
            Ok(mineral_script::ActionOutcome::Failed(e)) => bail!("动作 {name:?} 执行失败:{e}"),
            Err(_recv) => bail!("脚本线程已退出"),
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{Event, FinishReason};
    use mineral_script::{PropKey, PropValue, TrackFinishedReason};
    use mineral_test::song;

    use super::Notifier;

    // 脚本路的真实投递由 mineral-script 的 runtime 测试与 daemon e2e 覆盖;
    // 这里验 wire 路事件形状与同构映射。

    #[test]
    fn wire_lane_carries_events_in_order() -> color_eyre::Result<()> {
        use mineral_protocol::ToastKind;
        let (events_tx, mut events_rx) = tokio::sync::broadcast::channel(/*capacity*/ 8);
        let notifier = Notifier::new(events_tx, /*script*/ None);
        let s = song("1");
        notifier.track_finished(&s, FinishReason::Eof);
        notifier.property_changed(PropKey::PlayerVolume, &PropValue::Int(42));
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
                content: "下载不可用".to_owned(),
                id: None,
                ttl_secs: None,
            }
        );
        Ok(())
    }

    #[test]
    fn reason_and_value_mappings_are_isomorphic() {
        let pairs = [
            (FinishReason::Eof, TrackFinishedReason::Eof),
            (FinishReason::Skip, TrackFinishedReason::Skip),
            (FinishReason::Error, TrackFinishedReason::Error),
            (FinishReason::Stop, TrackFinishedReason::Stop),
        ];
        for (wire, script) in pairs {
            assert_eq!(super::to_script_reason(wire), script);
        }
        assert_eq!(
            super::to_wire_value(&PropValue::Str("x".to_owned())),
            mineral_protocol::PropValue::Str("x".to_owned())
        );
        assert_eq!(
            super::to_wire_value(&PropValue::None),
            mineral_protocol::PropValue::None
        );
    }
}

//! 脚本线程的生命周期句柄:spawn 移交 VM、Drop 优雅停机。

use color_eyre::eyre::WrapErr;
use mlua::Lua;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use crate::dispatch;
use crate::host::ScriptHost;
use crate::message::{ScriptMsg, ScriptSender};
use crate::watchdog::WatchdogConfig;

/// 脚本线程句柄。持有期间线程存活;Drop 发停机信号并 join
/// (等待在跑的回调结束,受看门狗硬阈值兜底,不会无限阻塞)。
#[derive(Debug)]
pub struct ScriptRuntime {
    /// 消息入口(克隆给 [`ScriptSender`])。
    tx: UnboundedSender<ScriptMsg>,

    /// 线程 join 句柄(Drop 时取走)。
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ScriptRuntime {
    /// 把已装 API、已 eval 用户脚本的 VM 移交给专用线程,启动主循环。
    ///
    /// # Params:
    ///   - `lua`: 目标 VM(`mlua::Lua` 是 `Send + !Sync`,随线程独占)
    ///   - `host`: 宿主句柄(与 daemon 侧共享注册表 / 通道)
    ///   - `watchdog`: 回调看门狗参数
    ///
    /// # Return:
    ///   线程句柄;OS 线程创建失败时为 `Err`。
    pub fn spawn(lua: Lua, host: ScriptHost, watchdog: WatchdogConfig) -> color_eyre::Result<Self> {
        let (tx, rx) = unbounded_channel();
        let handle = std::thread::Builder::new()
            .name("mineral-script".to_owned())
            .spawn(move || dispatch::run_loop(&lua, &host, &watchdog, rx))
            .wrap_err("spawn mineral-script thread")?;
        Ok(Self {
            tx,
            handle: Some(handle),
        })
    }

    /// daemon 侧的事件投递句柄(可任意克隆)。
    #[must_use]
    pub fn sender(&self) -> ScriptSender {
        ScriptSender(self.tx.clone())
    }
}

impl Drop for ScriptRuntime {
    fn drop(&mut self) {
        // 线程已退出时发送失败,照样走 join 收尸。
        let _ = self.tx.send(ScriptMsg::Stop);
        if let Some(handle) = self.handle.take()
            && handle.join().is_err()
        {
            // 主循环不含 panic 路径(workspace lints 禁止);真到这里只能记一笔。
            mineral_log::error!(target: "script", "mineral-script thread panicked");
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{Event, ToastKind};
    use mineral_test::{song, with_duration};
    use tokio::sync::mpsc::unbounded_channel;

    use super::*;
    use crate::host::install_api;
    use crate::message::{ScriptEvent, TrackFinishedReason};

    /// 宽松看门狗(测试回调都很短,阈值只兜底)。
    fn lax_watchdog() -> WatchdogConfig {
        WatchdogConfig::builder()
            .instruction_interval(10_000)
            .soft_wall(std::time::Duration::from_millis(200))
            .hard_wall(std::time::Duration::from_secs(1))
            .build()
    }

    /// 装 API + eval 脚本 + 移交线程,返回 (runtime, push 接收端)。
    fn spawn_with_script(
        script: &str,
    ) -> color_eyre::Result<(ScriptRuntime, tokio::sync::mpsc::UnboundedReceiver<Event>)> {
        let (cmd_tx, _cmd_rx) = unbounded_channel();
        let (push_tx, push_rx) = unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = Lua::new();
        install_api(&lua, &host)?;
        lua.load(script).exec()?;
        let runtime = ScriptRuntime::spawn(lua, host, lax_watchdog())?;
        Ok((runtime, push_rx))
    }

    /// drop runtime(Stop + join)后排干 push 通道。
    fn drain_after_stop(
        runtime: ScriptRuntime,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
    ) -> Vec<Event> {
        drop(runtime);
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    #[test]
    fn track_finished_reaches_lua_callback() -> color_eyre::Result<()> {
        let (runtime, mut push_rx) = spawn_with_script(
            r#"
            mineral.on("track_finished", function(args)
                local s = args.song
                mineral.ui.toast(s.id .. "/" .. s.title .. "/" .. s.duration_ms .. "/" .. args.reason)
            end)
            "#,
        )?;
        let song = with_duration(song("42"), /*duration_ms*/ 1500);
        runtime.sender().send(ScriptEvent::TrackFinished {
            song: Box::new(song.clone()),
            reason: TrackFinishedReason::Eof,
        });
        let events = drain_after_stop(runtime, &mut push_rx);
        let expected = format!("{}/{}/1500/eof", song.id.qualified(), song.name);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: expected,
                id: None,
            }]
        );
        Ok(())
    }

    #[test]
    fn download_completed_passes_path() -> color_eyre::Result<()> {
        let (runtime, mut push_rx) = spawn_with_script(
            r#"
            mineral.on("download_completed", function(args)
                mineral.ui.toast(args.path, { id = "dl" })
            end)
            "#,
        )?;
        runtime.sender().send(ScriptEvent::DownloadCompleted {
            song: Box::new(song("7")),
            path: std::path::PathBuf::from("/tmp/out.flac"),
        });
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: "/tmp/out.flac".to_owned(),
                id: Some("dl".to_owned()),
            }]
        );
        Ok(())
    }

    #[test]
    fn failing_callback_reports_error_toast_and_spares_others() -> color_eyre::Result<()> {
        let (runtime, mut push_rx) = spawn_with_script(
            r#"
            mineral.on("track_finished", function() error("boom") end)
            mineral.on("track_finished", function() mineral.ui.toast("still alive") end)
            "#,
        )?;
        runtime.sender().send(ScriptEvent::TrackFinished {
            song: Box::new(song("1")),
            reason: TrackFinishedReason::Skip,
        });
        let events = drain_after_stop(runtime, &mut push_rx);
        let (Some(first), Some(second)) = (events.first(), events.get(1)) else {
            color_eyre::eyre::bail!("期望 2 条事件(错误 toast + 后续回调),实得 {events:?}");
        };
        assert_eq!(
            *first,
            Event::Toast {
                kind: ToastKind::Error,
                content: "脚本 track_finished 回调出错,详见日志".to_owned(),
                id: Some("script.error".to_owned()),
            },
            "失败回调先报错误 toast"
        );
        assert_eq!(
            *second,
            Event::Toast {
                kind: ToastKind::Info,
                content: "still alive".to_owned(),
                id: None,
            },
            "同事件后续回调不被失败者拖死"
        );
        Ok(())
    }

    #[test]
    fn property_changed_reaches_observer_and_updates_cache() -> color_eyre::Result<()> {
        use crate::message::{PropKey, PropValue};
        let (runtime, mut push_rx) = spawn_with_script(
            r#"
            mineral.observe("player.volume", function(v)
                mineral.ui.toast("vol=" .. v, { id = "vol" })
            end)
            "#,
        )?;
        runtime.sender().send(ScriptEvent::PropertyChanged {
            key: PropKey::PlayerVolume,
            value: PropValue::Int(55),
        });
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: "vol=55".to_owned(),
                id: Some("vol".to_owned()),
            }]
        );
        Ok(())
    }

    #[test]
    fn invoke_action_round_trips_outcomes() -> color_eyre::Result<()> {
        use crate::message::ActionOutcome;
        let (runtime, mut push_rx) = spawn_with_script(
            r#"
            mineral.action("my.toast", function(ctx) mineral.ui.toast("acted") end)
            mineral.action("my.boom", function(ctx) error("nope") end)
            "#,
        )?;
        let sender = runtime.sender();
        let done = sender
            .invoke_action("my.toast".to_owned())
            .blocking_recv()?;
        assert_eq!(done, ActionOutcome::Done);
        let missing = sender.invoke_action("my.gone".to_owned()).blocking_recv()?;
        assert_eq!(missing, ActionOutcome::NotFound);
        let failed = sender.invoke_action("my.boom".to_owned()).blocking_recv()?;
        assert!(
            matches!(failed, ActionOutcome::Failed(ref e) if e.contains("nope")),
            "实得 {failed:?}"
        );
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: "acted".to_owned(),
                id: None,
            }],
            "动作失败不推 error toast(结果经回执返回)"
        );
        Ok(())
    }

    #[test]
    fn drop_joins_thread_gracefully() -> color_eyre::Result<()> {
        let (runtime, mut push_rx) = spawn_with_script("-- 无注册")?;
        runtime.sender().send(ScriptEvent::TrackFinished {
            song: Box::new(song("9")),
            reason: TrackFinishedReason::Stop,
        });
        let sender = runtime.sender();
        let events = drain_after_stop(runtime, &mut push_rx);
        assert!(events.is_empty(), "无注册回调,不该有任何推送");
        // 线程已 join:再投递只是静默丢,不 panic 不阻塞。
        sender.send(ScriptEvent::TrackFinished {
            song: Box::new(song("10")),
            reason: TrackFinishedReason::Eof,
        });
        Ok(())
    }
}

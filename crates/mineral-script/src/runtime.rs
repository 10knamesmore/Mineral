//! 脚本线程的生命周期句柄:spawn 移交 VM、Drop 优雅停机。

use color_eyre::eyre::WrapErr;
use mlua::Lua;

use crate::dispatch;
use crate::host::ScriptHost;
use crate::message::{ScriptMsg, ScriptSender};
use crate::watchdog::WatchdogConfig;

/// 脚本线程句柄。持有期间线程存活;Drop 发停机信号并 join
/// (等待在跑的回调结束,受看门狗硬阈值兜底,不会无限阻塞)。
#[derive(Debug)]
pub struct ScriptRuntime {
    /// 消息入口(克隆给 [`ScriptSender`])。
    tx: std::sync::mpsc::Sender<ScriptMsg>,

    /// 线程 join 句柄(Drop 时取走)。
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ScriptRuntime {
    /// 把已装 API、已 eval 用户脚本的 VM 移交给专用线程,启动主循环,
    /// 并把消息入口挂进 `sender`(daemon 各处持有的间接句柄从此指向本线程)。
    ///
    /// # Params:
    ///   - `lua`: 目标 VM(`mlua::Lua` 是 `Send + !Sync`,随线程独占)
    ///   - `host`: 宿主句柄(与 daemon 侧共享注册表 / 通道)
    ///   - `watchdog`: 回调看门狗参数
    ///   - `sender`: daemon 侧投递句柄(spawn 成功即 attach;热重载复用同一个)
    ///
    /// # Return:
    ///   线程句柄;OS 线程创建失败时为 `Err`(`sender` 保持原挂接不动)。
    pub fn spawn(
        lua: Lua,
        host: ScriptHost,
        watchdog: WatchdogConfig,
        sender: &ScriptSender,
    ) -> color_eyre::Result<Self> {
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("mineral-script".to_owned())
            .spawn(move || dispatch::run_loop(&lua, &host, &watchdog, &rx))
            .wrap_err("spawn mineral-script thread")?;
        sender.attach(tx.clone());
        Ok(Self {
            tx,
            handle: Some(handle),
        })
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

    /// 装 API + eval 脚本 + 移交线程,返回 (runtime, 投递句柄, push 接收端)。
    fn spawn_with_script(
        script: &str,
    ) -> color_eyre::Result<(
        ScriptRuntime,
        ScriptSender,
        tokio::sync::mpsc::UnboundedReceiver<Event>,
    )> {
        let (cmd_tx, _cmd_rx) = unbounded_channel();
        let (push_tx, push_rx) = unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = Lua::new();
        install_api(&lua, &host)?;
        lua.load(script).exec()?;
        let sender = ScriptSender::detached();
        let runtime = ScriptRuntime::spawn(lua, host, lax_watchdog(), &sender)?;
        Ok((runtime, sender, push_rx))
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
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.on("track_finished", function(args)
                local s = args.song
                mineral.ui.toast(s.id .. "/" .. s.title .. "/" .. s.duration_ms .. "/" .. args.reason)
            end)
            "#,
        )?;
        let song = with_duration(song("42"), /*duration_ms*/ 1500);
        sender.send(ScriptEvent::TrackFinished {
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
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    #[test]
    fn download_completed_passes_path() -> color_eyre::Result<()> {
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.on("download_completed", function(args)
                mineral.ui.toast(args.path, { id = "dl" })
            end)
            "#,
        )?;
        sender.send(ScriptEvent::DownloadCompleted {
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
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    #[test]
    fn failing_callback_reports_error_toast_and_spares_others() -> color_eyre::Result<()> {
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.on("track_finished", function() error("boom") end)
            mineral.on("track_finished", function() mineral.ui.toast("still alive") end)
            "#,
        )?;
        sender.send(ScriptEvent::TrackFinished {
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
                ttl_secs: None,
            },
            "失败回调先报错误 toast"
        );
        assert_eq!(
            *second,
            Event::Toast {
                kind: ToastKind::Info,
                content: "still alive".to_owned(),
                id: None,
                ttl_secs: None,
            },
            "同事件后续回调不被失败者拖死"
        );
        Ok(())
    }

    #[test]
    fn property_changed_reaches_observer_and_updates_cache() -> color_eyre::Result<()> {
        use crate::message::{PropKey, PropValue};
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.observe("player.volume", function(v)
                mineral.ui.toast("vol=" .. v, { id = "vol" })
            end)
            "#,
        )?;
        sender.send(ScriptEvent::PropertyChanged {
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
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    #[test]
    fn invoke_action_round_trips_outcomes() -> color_eyre::Result<()> {
        use crate::message::ActionOutcome;
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.action("my.toast", function(ctx) mineral.ui.toast("acted") end)
            mineral.action("my.boom", function(ctx) error("nope") end)
            "#,
        )?;
        let done = sender
            .invoke_action("my.toast".to_owned(), /*ctx*/ None)
            .blocking_recv()?;
        assert_eq!(done, ActionOutcome::Done);
        let missing = sender
            .invoke_action("my.gone".to_owned(), /*ctx*/ None)
            .blocking_recv()?;
        assert_eq!(missing, ActionOutcome::NotFound);
        let failed = sender
            .invoke_action("my.boom".to_owned(), /*ctx*/ None)
            .blocking_recv()?;
        assert!(
            matches!(failed, ActionOutcome::Failed(ref e) if e.contains("nope")),
            "实得 {failed:?}"
        );
        assert!(
            matches!(failed, ActionOutcome::Failed(ref e) if !e.contains('\n')),
            "回执错误必须单行(traceback 进日志,不进 toast):实得 {failed:?}"
        );
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: "acted".to_owned(),
                id: None,
                ttl_secs: None,
            }],
            "动作失败不推 error toast(结果经回执返回)"
        );
        Ok(())
    }

    #[test]
    fn invoke_action_exposes_ctx_fields_to_lua() -> color_eyre::Result<()> {
        use mineral_model::{PlaylistId, SourceKind};
        use mineral_protocol::{KeyContext, PlaylistRef, ViewKind};

        use crate::message::ActionOutcome;
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.action("show.ctx", function(ctx)
                local view = ctx.view or "none"
                local sel = ctx.selected_song and ctx.selected_song.title or "none"
                local pl = ctx.selected_playlist and ctx.selected_playlist.name or "none"
                local np = ctx.now_playing and ctx.now_playing.id or "none"
                local loved = tostring(ctx.selected_loved)
                local q = ctx.search_query or "none"
                mineral.ui.toast(view .. "/" .. sel .. "/" .. pl .. "/" .. np .. "/" .. loved .. "/" .. q)
            end)
            "#,
        )?;
        // TUI 触发:带上下文,字段进 ctx 表
        let ctx = KeyContext::builder()
            .view(ViewKind::Tracks)
            .selected_song(Some(Box::new(song("11"))))
            .selected_playlist(Some(PlaylistRef {
                id: PlaylistId::new(SourceKind::NETEASE, "p1"),
                name: "日常".to_owned(),
            }))
            .now_playing(Some(Box::new(song("22"))))
            .selected_loved(Some(true))
            .search_query(Some("雨".to_owned()))
            .build();
        let done = sender
            .invoke_action("show.ctx".to_owned(), Some(ctx))
            .blocking_recv()?;
        assert_eq!(done, ActionOutcome::Done);
        // CLI 触发:无上下文,ctx 是空表,字段全 nil
        let done = sender
            .invoke_action("show.ctx".to_owned(), /*ctx*/ None)
            .blocking_recv()?;
        assert_eq!(done, ActionOutcome::Done);
        let events = drain_after_stop(runtime, &mut push_rx);
        let contents = events
            .iter()
            .map(|e| match e {
                Event::Toast { content, .. } => content.clone(),
                other => format!("{other:?}"),
            })
            .collect::<Vec<String>>();
        let sel_title = song("11").name;
        assert_eq!(
            contents,
            vec![
                format!("tracks/{sel_title}/日常/netease:22/true/雨"),
                "none/none/none/none/nil/none".to_owned(),
            ],
            "ctx 字段按蛇形名进表(歌/歌单是子表);无 ctx 时空表"
        );
        Ok(())
    }

    /// 装 API + eval + 移交线程,返回 (runtime, cmd 接收端, push 接收端)。
    /// 查询类 API 测试用:断言 cmd、手动 resolve 模拟 daemon 泵。
    fn spawn_with_cmds(
        script: &str,
    ) -> color_eyre::Result<(
        ScriptRuntime,
        ScriptSender,
        tokio::sync::mpsc::UnboundedReceiver<crate::ScriptCmd>,
        tokio::sync::mpsc::UnboundedReceiver<Event>,
    )> {
        let (cmd_tx, cmd_rx) = unbounded_channel();
        let (push_tx, push_rx) = unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = Lua::new();
        install_api(&lua, &host)?;
        lua.load(script).exec()?;
        let sender = ScriptSender::detached();
        let runtime = ScriptRuntime::spawn(lua, host, lax_watchdog(), &sender)?;
        Ok((runtime, sender, cmd_rx, push_rx))
    }

    #[test]
    fn store_get_resolves_callback_with_value() -> color_eyre::Result<()> {
        use crate::message::{ResolveValue, ScriptCmd};
        let (runtime, sender, mut cmd_rx, mut push_rx) = spawn_with_cmds(
            r#"
            mineral.store.get("netease:1", "plugin.x", function(v, err)
                mineral.ui.toast(tostring(v) .. "/" .. tostring(err))
            end)
            "#,
        )?;
        // eval 时已发出查询命令;断言形状并取 query id
        let cmd = cmd_rx.try_recv()?;
        let ScriptCmd::StoreGet { song, key, query } = cmd else {
            color_eyre::eyre::bail!("期望 StoreGet,实得 {cmd:?}");
        };
        assert_eq!(song.qualified(), "netease:1");
        assert_eq!(key, "plugin.x");
        // 模拟 daemon 泵回投结果
        sender.resolve(
            query,
            ResolveValue::Store(mineral_protocol::StoreValue::Int(7)),
        );
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: "7/nil".to_owned(),
                id: None,
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    #[test]
    fn store_set_and_inc_emit_structured_cmds() -> color_eyre::Result<()> {
        use crate::message::ScriptCmd;
        use mineral_protocol::StoreValue;
        let (runtime, _sender, mut cmd_rx, _push_rx) = spawn_with_cmds(
            r#"
            mineral.store.set("netease:2", "plugin.s", "文本")
            mineral.store.set("netease:2", "plugin.b", true)
            mineral.store.set("netease:2", "plugin.f", 2.5)
            mineral.store.set("netease:2", "plugin.gone", nil)
            mineral.store.inc("netease:2", "plugin.n", 3)
            "#,
        )?;
        drop(runtime);
        let mut values = Vec::new();
        while let Ok(cmd) = cmd_rx.try_recv() {
            values.push(cmd);
        }
        let kinds = values
            .iter()
            .map(|c| match c {
                ScriptCmd::StoreSet { value, .. } => format!("set:{value:?}"),
                ScriptCmd::StoreInc { delta, query, .. } => {
                    format!("inc:{delta}:{}", query.is_some())
                }
                other => format!("{other:?}"),
            })
            .collect::<Vec<String>>();
        assert_eq!(
            kinds,
            vec![
                format!("set:{:?}", StoreValue::Text("文本".to_owned())),
                format!("set:{:?}", StoreValue::Bool(true)),
                format!("set:{:?}", StoreValue::Real(2.5)),
                format!("set:{:?}", StoreValue::Nil),
                "inc:3:false".to_owned(),
            ],
            "Lua 值按类型映射;inc 不带回调则无 query"
        );
        Ok(())
    }

    #[test]
    fn resolve_error_passes_nil_and_message() -> color_eyre::Result<()> {
        use crate::message::{ResolveValue, ScriptCmd};
        let (runtime, sender, mut cmd_rx, mut push_rx) = spawn_with_cmds(
            r#"
            mineral.store.inc("netease:3", "plugin.s", 1, function(v, err)
                mineral.ui.toast(tostring(v) .. "/" .. tostring(err))
            end)
            "#,
        )?;
        let cmd = cmd_rx.try_recv()?;
        let ScriptCmd::StoreInc {
            query: Some(query), ..
        } = cmd
        else {
            color_eyre::eyre::bail!("期望带回调的 StoreInc,实得 {cmd:?}");
        };
        sender.resolve(query, ResolveValue::Error("不能自增".to_owned()));
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: "nil/不能自增".to_owned(),
                id: None,
                ttl_secs: None,
            }],
            "失败回调收 (nil, err)"
        );
        Ok(())
    }

    #[test]
    fn queue_list_resolves_song_array() -> color_eyre::Result<()> {
        use crate::message::{ResolveValue, ScriptCmd};
        let (runtime, sender, mut cmd_rx, mut push_rx) = spawn_with_cmds(
            r#"
            mineral.queue.list(function(q, err)
                mineral.ui.toast(#q .. ":" .. q[1].title .. ":" .. q[2].id)
            end)
            "#,
        )?;
        let cmd = cmd_rx.try_recv()?;
        let ScriptCmd::QueueList { query } = cmd else {
            color_eyre::eyre::bail!("期望 QueueList,实得 {cmd:?}");
        };
        let (first, second) = (song("1"), song("2"));
        let songs = vec![first.clone(), second.clone()];
        let want = format!("2:{}:{}", first.name, second.id.qualified());
        sender.resolve(query, ResolveValue::Songs(songs));
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: want,
                id: None,
                ttl_secs: None,
            }],
            "队列按序投影成 Lua 数组"
        );
        Ok(())
    }

    #[test]
    fn library_apis_emit_cmds_and_resolve() -> color_eyre::Result<()> {
        use crate::message::{PlaylistBrief, ResolveValue, ScriptCmd};
        use mineral_model::{PlaylistId, SourceKind};
        let (runtime, sender, mut cmd_rx, mut push_rx) = spawn_with_cmds(
            r#"
            mineral.library.playlists(function(ps, err)
                mineral.ui.toast(ps[1].id .. ":" .. ps[1].name .. ":" .. ps[1].track_count)
            end)
            mineral.library.tracks("netease:p9", function(ts, err) end)
            mineral.library.love("netease:5", true)
            mineral.library.love("netease:6", false)
            "#,
        )?;
        let first = cmd_rx.try_recv()?;
        let ScriptCmd::LibraryPlaylists { query } = first else {
            color_eyre::eyre::bail!("期望 LibraryPlaylists,实得 {first:?}");
        };
        let tracks_cmd = cmd_rx.try_recv()?;
        let ScriptCmd::LibraryTracks { playlist, .. } = &tracks_cmd else {
            color_eyre::eyre::bail!("期望 LibraryTracks,实得 {tracks_cmd:?}");
        };
        assert_eq!(playlist.qualified(), "netease:p9");
        assert_eq!(
            cmd_rx.try_recv()?,
            ScriptCmd::SetLoved {
                song: mineral_model::SongId::new(SourceKind::NETEASE, "5"),
                loved: true,
            }
        );
        assert_eq!(
            cmd_rx.try_recv()?,
            ScriptCmd::SetLoved {
                song: mineral_model::SongId::new(SourceKind::NETEASE, "6"),
                loved: false,
            }
        );
        sender.resolve(
            query,
            ResolveValue::Playlists(vec![PlaylistBrief {
                id: PlaylistId::new(SourceKind::NETEASE, "p1"),
                name: "日常".to_owned(),
                track_count: 42,
            }]),
        );
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: "netease:p1:日常:42".to_owned(),
                id: None,
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    /// 在 `deadline` 内轮询 push 通道直到收满 `n` 条(或超时返回已收的)。
    fn wait_events(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>,
        n: usize,
        deadline: std::time::Duration,
    ) -> Vec<Event> {
        let until = std::time::Instant::now() + deadline;
        let mut events = Vec::new();
        while events.len() < n && std::time::Instant::now() < until {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(_empty) => std::thread::sleep(std::time::Duration::from_millis(5)),
            }
        }
        events
    }

    #[test]
    fn timer_after_fires_exactly_once() -> color_eyre::Result<()> {
        let (runtime, _sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.timer.after(20, function() mineral.ui.toast("dinged") end)
            "#,
        )?;
        let events = wait_events(
            &mut push_rx,
            /*n*/ 1,
            std::time::Duration::from_millis(2000),
        );
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: "dinged".to_owned(),
                id: None,
                ttl_secs: None,
            }]
        );
        // 一次性:再等一阵不该有第二发
        std::thread::sleep(std::time::Duration::from_millis(80));
        let extra = drain_after_stop(runtime, &mut push_rx);
        assert!(extra.is_empty(), "after 只触发一次,实得 {extra:?}");
        Ok(())
    }

    #[test]
    fn timer_every_repeats_and_kill_stops() -> color_eyre::Result<()> {
        let (runtime, _sender, mut push_rx) = spawn_with_script(
            r#"
            local t
            local n = 0
            t = mineral.timer.every(15, function()
                n = n + 1
                mineral.ui.toast("tick" .. n)
                if n >= 2 then t:kill() end
            end)
            "#,
        )?;
        let events = wait_events(
            &mut push_rx,
            /*n*/ 2,
            std::time::Duration::from_millis(2000),
        );
        assert_eq!(events.len(), 2, "every 周期触发,实得 {events:?}");
        std::thread::sleep(std::time::Duration::from_millis(80));
        let extra = drain_after_stop(runtime, &mut push_rx);
        assert!(extra.is_empty(), "kill 后不再触发,实得 {extra:?}");
        Ok(())
    }

    #[test]
    fn timer_stop_freezes_and_resume_continues() -> color_eyre::Result<()> {
        let (runtime, _sender, mut push_rx) = spawn_with_script(
            r#"
            local t = mineral.timer.every(15, function() mineral.ui.toast("beat") end)
            t:stop()
            mineral.timer.after(60, function() t:resume() end)
            "#,
        )?;
        // stop 期间(前 ~60ms)不触发
        let early = wait_events(
            &mut push_rx,
            /*n*/ 1,
            std::time::Duration::from_millis(40),
        );
        assert!(early.is_empty(), "stop 期间不该触发,实得 {early:?}");
        // resume 后恢复周期触发
        let after_resume = wait_events(
            &mut push_rx,
            /*n*/ 1,
            std::time::Duration::from_millis(2000),
        );
        assert_eq!(after_resume.len(), 1, "resume 后应恢复触发");
        drop(runtime);
        Ok(())
    }

    #[test]
    fn script_binds_round_trip_via_sender() -> color_eyre::Result<()> {
        use mineral_protocol::ScriptBind;
        let (_runtime, sender, _push_rx) = spawn_with_script(
            r#"
            mineral.bind("X", function() mineral.ui.toast("bound") end)
            "#,
        )?;
        let binds = sender.script_binds().blocking_recv()?;
        assert_eq!(
            binds,
            vec![ScriptBind {
                key: "X".to_owned(),
                action: "bind#1".to_owned(),
            }]
        );
        // bind 的匿名动作经触发链可调(复用 action 通道)。
        let done = sender
            .invoke_action("bind#1".to_owned(), /*ctx*/ None)
            .blocking_recv()?;
        assert_eq!(done, crate::message::ActionOutcome::Done);
        Ok(())
    }

    #[test]
    fn drop_joins_thread_gracefully() -> color_eyre::Result<()> {
        let (runtime, sender, mut push_rx) = spawn_with_script("-- 无注册")?;
        sender.send(ScriptEvent::TrackFinished {
            song: Box::new(song("9")),
            reason: TrackFinishedReason::Stop,
        });
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

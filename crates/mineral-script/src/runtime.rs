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
    use mineral_protocol::{Event, TextSpan, ToastKind};
    use mineral_test::{endserenading, song, with_duration};
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

    /// 把 toast 的 spans 内容拼回纯文本(断言用)。
    fn flat(content: &[TextSpan]) -> String {
        content.iter().map(|s| s.text.as_str()).collect::<String>()
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
    fn track_started_reaches_lua_callback() -> color_eyre::Result<()> {
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.on("track_started", function(args)
                mineral.ui.toast("start:" .. args.song.id .. "/" .. args.song.title)
            end)
            "#,
        )?;
        let song = first_track()?;
        sender.send(ScriptEvent::TrackStarted {
            song: Box::new(song.clone()),
        });
        let events = drain_after_stop(runtime, &mut push_rx);
        let expected = format!("start:{}/{}", song.id.qualified(), song.name);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: vec![TextSpan::plain(expected)],
                id: None,
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    /// 《EndSerenading》首曲(LoveLetterTypewriter,艺人 Mineral)。
    fn first_track() -> color_eyre::Result<mineral_model::Song> {
        endserenading(1)
            .pop()
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 应至少给一首"))
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
                content: vec![TextSpan::plain(expected)],
                id: None,
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    #[test]
    fn download_completed_passes_path_quality_format() -> color_eyre::Result<()> {
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.on("download_completed", function(args)
                mineral.ui.toast(args.path .. "/" .. args.quality .. "/" .. args.format, { id = "dl" })
            end)
            "#,
        )?;
        sender.send(ScriptEvent::DownloadCompleted {
            song: Box::new(first_track()?),
            path: std::path::PathBuf::from("/tmp/LoveLetterTypewriter.flac"),
            quality: mineral_model::BitRate::Lossless,
            format: mineral_model::AudioFormat::Flac,
        });
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: vec![TextSpan::plain(
                    "/tmp/LoveLetterTypewriter.flac/lossless/flac"
                )],
                id: Some("dl".to_owned()),
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    /// Song 投影携带 artists / album / cover_url(Option 字段缺席为 nil),
    /// 以及 source / url(未 seed 模板时 url 为 nil)。
    #[test]
    fn song_projection_carries_artists_album_and_urls() -> color_eyre::Result<()> {
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.on("track_started", function(args)
                local s = args.song
                mineral.ui.toast(table.concat(s.artists, ",") .. "/" .. tostring(s.album)
                    .. "/" .. tostring(s.cover_url) .. "/" .. tostring(s.source_url)
                    .. "/" .. s.source .. "/" .. tostring(s.url))
            end)
            "#,
        )?;
        let mut s = first_track()?;
        s.album = Some(mineral_model::AlbumRef {
            id: mineral_model::AlbumId::new(mineral_model::SourceKind::NETEASE, "es"),
            name: "EndSerenading".to_owned(),
        });
        sender.send(ScriptEvent::TrackStarted { song: Box::new(s) });
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: vec![TextSpan::plain("Mineral/EndSerenading/nil/nil/netease/nil")],
                id: None,
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    /// seed 网页模板后 Song 投影的 url 按源模板拼出(`{id}` 填裸 id)。
    #[test]
    fn song_projection_url_uses_seeded_template() -> color_eyre::Result<()> {
        let (cmd_tx, _cmd_rx) = unbounded_channel();
        let (push_tx, mut push_rx) = unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = Lua::new();
        install_api(&lua, &host)?;
        crate::host::seed_web_url_templates(
            &lua,
            vec![(
                "netease".to_owned(),
                Some("https://x.example/song?id={id}".to_owned()),
                None,
            )],
        )?;
        lua.load(
            r#"
            mineral.on("track_started", function(args)
                mineral.ui.toast(tostring(args.song.url))
            end)
            "#,
        )
        .exec()?;
        let sender = ScriptSender::detached();
        let runtime = ScriptRuntime::spawn(lua, host, lax_watchdog(), &sender)?;
        let song = first_track()?;
        let raw = song.id.value().to_owned();
        sender.send(ScriptEvent::TrackStarted {
            song: Box::new(song),
        });
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: vec![TextSpan::plain(format!("https://x.example/song?id={raw}"))],
                id: None,
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    /// 复制模板端到端:registry 函数按下标执行,song / playlist 两种 ctx 投影
    /// 正确;下标无函数回人读错误。
    #[test]
    fn render_copy_template_executes_registry_function() -> color_eyre::Result<()> {
        let (cmd_tx, _cmd_rx) = unbounded_channel();
        let (push_tx, _push_rx) = unbounded_channel();
        let host = ScriptHost::new(cmd_tx, push_tx);
        let lua = Lua::new();
        install_api(&lua, &host)?;
        // 模拟 config 管线摘好的函数表:1 = song 模板,2 = playlist 模板。
        let fns = lua.create_table()?;
        fns.set(
            1,
            lua.load("function(s) return s.title .. '|' .. s.source end")
                .eval::<mlua::Function>()?,
        )?;
        fns.set(
            2,
            lua.load("function(p) return p.name .. '|' .. #p.songs .. '|' .. p.songs[1].title end")
                .eval::<mlua::Function>()?,
        )?;
        lua.set_named_registry_value(mineral_config::COPY_TEMPLATE_FNS, fns)?;
        let sender = ScriptSender::detached();
        let _runtime = ScriptRuntime::spawn(lua, host, lax_watchdog(), &sender)?;

        let song = first_track()?;
        let got = sender
            .render_copy_template(
                /*index*/ 0,
                mineral_protocol::CopyTemplateCtx::Song(Box::new(song.clone())),
            )
            .blocking_recv()?;
        assert_eq!(got, Ok(format!("{}|netease", song.name)));

        let playlist = mineral_model::Playlist::builder()
            .id(mineral_model::PlaylistId::new(
                mineral_model::SourceKind::NETEASE,
                "p1",
            ))
            .name("歌单甲".to_owned())
            .track_count(1)
            .songs(vec![song.clone()])
            .build();
        let got = sender
            .render_copy_template(
                /*index*/ 1,
                mineral_protocol::CopyTemplateCtx::Playlist(Box::new(playlist)),
            )
            .blocking_recv()?;
        assert_eq!(got, Ok(format!("歌单甲|1|{}", song.name)));

        let got = sender
            .render_copy_template(
                /*index*/ 9,
                mineral_protocol::CopyTemplateCtx::Song(Box::new(song)),
            )
            .blocking_recv()?;
        assert!(
            got.as_ref().is_err_and(|e| e.contains("模板 #9")),
            "下标无函数应回人读错误,实得 {got:?}"
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
                content: vec![TextSpan::plain("脚本 track_finished 回调出错,详见日志")],
                id: Some("script.error".to_owned()),
                ttl_secs: None,
            },
            "失败回调先报错误 toast"
        );
        assert_eq!(
            *second,
            Event::Toast {
                kind: ToastKind::Info,
                content: vec![TextSpan::plain("still alive")],
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
                content: vec![TextSpan::plain("vol=55")],
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
                content: vec![TextSpan::plain("acted")],
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
                Event::Toast { content, .. } => flat(content),
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
                content: vec![TextSpan::plain("7/nil")],
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
                content: vec![TextSpan::plain("nil/不能自增")],
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
                content: vec![TextSpan::plain(want)],
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
                content: vec![TextSpan::plain("netease:p1:日常:42")],
                id: None,
                ttl_secs: None,
            }]
        );
        Ok(())
    }

    #[test]
    fn library_search_emits_cmd_and_resolves() -> color_eyre::Result<()> {
        use crate::message::{ResolveValue, ScriptCmd};
        use mineral_model::SourceKind;
        let (runtime, sender, mut cmd_rx, mut push_rx) = spawn_with_cmds(
            r#"
            mineral.library.search("雨", function(songs, err)
                mineral.ui.toast(#songs .. "/" .. tostring(err))
            end)
            mineral.library.search("雪", { source = "netease", offset = 10, limit = 5 },
                function(songs, err) end)
            "#,
        )?;
        // 双形态:无 opts 走默认分页,有 opts 逐字段透传
        let first = cmd_rx.try_recv()?;
        let ScriptCmd::LibrarySearch {
            term,
            source,
            offset,
            limit,
            query,
        } = first
        else {
            color_eyre::eyre::bail!("期望 LibrarySearch,实得 {first:?}");
        };
        assert_eq!(term, "雨");
        assert_eq!(source, None);
        assert_eq!((offset, limit), (0, 30), "缺省分页 = offset 0 / limit 30");
        let second = cmd_rx.try_recv()?;
        let ScriptCmd::LibrarySearch {
            term,
            source,
            offset,
            limit,
            ..
        } = second
        else {
            color_eyre::eyre::bail!("期望 LibrarySearch,实得 {second:?}");
        };
        assert_eq!(term, "雪");
        assert_eq!(source, Some(SourceKind::NETEASE));
        assert_eq!((offset, limit), (10, 5));
        // 模拟 daemon 泵回投命中
        sender.resolve(query, ResolveValue::Songs(vec![song("1"), song("2")]));
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: vec![TextSpan::plain("2/nil")],
                id: None,
                ttl_secs: None,
            }],
            "search 回调收 (songs, nil)"
        );
        Ok(())
    }

    /// 拦截测试用的入参快照(固定歌 + 远端 URL)。
    fn hook_ctx() -> color_eyre::Result<crate::hooks::HookContext> {
        use color_eyre::eyre::WrapErr;
        let target = song("1");
        let play_url = mineral_model::PlayUrl {
            song_id: target.id.clone(),
            url: "https://example.com/a.flac"
                .parse::<mineral_model::MediaUrl>()
                .wrap_err("parse url")?,
            bitrate_bps: 0,
            quality: mineral_model::BitRate::Exhigh,
            size: 0,
            format: mineral_model::AudioFormat::Flac,
            bit_depth: None,
        };
        Ok(crate::hooks::HookContext::new(target, play_url))
    }

    #[tokio::test]
    async fn intercept_detached_sender_continues() -> color_eyre::Result<()> {
        use crate::hooks::{HookDecision, HookKind};
        // 未挂线程:拦截天然不存在,立即放行(不等超时)。
        let sender = ScriptSender::detached();
        let decision = sender
            .intercept(
                HookKind::BeforePlay,
                hook_ctx()?,
                std::time::Duration::from_secs(60),
            )
            .await;
        assert_eq!(decision, HookDecision::Continue);
        Ok(())
    }

    #[tokio::test]
    async fn intercept_no_hooks_registered_continues() -> color_eyre::Result<()> {
        use crate::hooks::{HookDecision, HookKind};
        // 有脚本但未注册 hook:脚本线程立即回执放行。
        let (runtime, sender, _push_rx) = spawn_with_script("-- 无 hook")?;
        let decision = sender
            .intercept(
                HookKind::BeforeDownload,
                hook_ctx()?,
                std::time::Duration::from_secs(5),
            )
            .await;
        assert_eq!(decision, HookDecision::Continue);
        drop(runtime);
        Ok(())
    }

    #[tokio::test]
    async fn intercept_timeout_falls_back_to_continue() -> color_eyre::Result<()> {
        use crate::hooks::{HookDecision, HookKind};
        // 挂一条没人消费的通道:回执永不到 → 墙钟超时放行。
        let sender = ScriptSender::detached();
        let (tx, rx) = std::sync::mpsc::channel();
        sender.attach(tx);
        let started = std::time::Instant::now();
        let decision = sender
            .intercept(
                HookKind::BeforePlay,
                hook_ctx()?,
                std::time::Duration::from_millis(50),
            )
            .await;
        assert_eq!(decision, HookDecision::Continue, "超时必须放行");
        assert!(
            started.elapsed() >= std::time::Duration::from_millis(50),
            "放行必须等满软超时"
        );
        drop(rx);
        Ok(())
    }

    #[tokio::test]
    async fn hook_rewrite_returns_structured_spec() -> color_eyre::Result<()> {
        use crate::hooks::{HookDecision, HookKind};
        let (runtime, sender, _push_rx) = spawn_with_script(
            r#"
            mineral.hook("before_play", function(ctx)
                -- ctx 字段可读:据原音质决定改写
                if ctx.quality == "exhigh" then
                    return { url = "https://fallback.example/b.flac", quality = "standard" }
                end
            end)
            "#,
        )?;
        let decision = sender
            .intercept(
                HookKind::BeforePlay,
                hook_ctx()?,
                std::time::Duration::from_secs(5),
            )
            .await;
        let HookDecision::Rewrite(spec) = decision else {
            color_eyre::eyre::bail!("期望 Rewrite,实得 {decision:?}");
        };
        assert_eq!(
            spec.new_url().map(ToString::to_string),
            Some("https://fallback.example/b.flac".to_owned())
        );
        assert_eq!(spec.new_quality(), Some(mineral_model::BitRate::Standard));
        drop(runtime);
        Ok(())
    }

    #[tokio::test]
    async fn hook_skip_and_short_circuit() -> color_eyre::Result<()> {
        use crate::hooks::{HookDecision, HookKind};
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.hook("before_download", function(ctx)
                return { skip = "网络探测失败:" .. ctx.kind }
            end)
            -- 第二个 hook 不应被调到(首个非放行短路)
            mineral.hook("before_download", function(ctx)
                mineral.ui.toast("不该出现")
                return nil
            end)
            "#,
        )?;
        let decision = sender
            .intercept(
                HookKind::BeforeDownload,
                hook_ctx()?,
                std::time::Duration::from_secs(5),
            )
            .await;
        assert_eq!(
            decision,
            HookDecision::Skip {
                reason: "网络探测失败:before_download".to_owned()
            }
        );
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(events, vec![], "短路后第二个 hook 不应被调用");
        Ok(())
    }

    #[tokio::test]
    async fn hook_false_skips_and_errors_fall_through() -> color_eyre::Result<()> {
        use crate::hooks::{HookDecision, HookKind};
        // 三连注册:Lua 错误 → 放行继续走下一个;非法返回值 → 同;false → 跳过。
        let (runtime, sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.hook("before_play", function(ctx) error("炸") end)
            mineral.hook("before_play", function(ctx) return 42 end)
            mineral.hook("before_play", function(ctx) return false end)
            "#,
        )?;
        let decision = sender
            .intercept(
                HookKind::BeforePlay,
                hook_ctx()?,
                std::time::Duration::from_secs(5),
            )
            .await;
        assert_eq!(
            decision,
            HookDecision::Skip {
                reason: "脚本跳过".to_owned()
            },
            "前两个失败 hook 按放行跳过,第三个 false 生效"
        );
        // 两次失败各推一条 error toast(同 id 顶替)。
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(events.len(), 2, "两个失败 hook 各报一条 error toast");
        Ok(())
    }

    #[tokio::test]
    async fn hook_unknown_name_is_script_error() -> color_eyre::Result<()> {
        let (cmd_tx, _cmd_rx) = unbounded_channel();
        let (push_tx, _push_rx) = unbounded_channel();
        let host = crate::host::ScriptHost::new(cmd_tx, push_tx);
        let lua = Lua::new();
        install_api(&lua, &host)?;
        let result = lua
            .load(r#"mineral.hook("after_play", function() end)"#)
            .exec();
        assert!(result.is_err(), "未知 hook 名必须当场报错");
        Ok(())
    }

    #[test]
    fn spawn_emits_cmd_and_resolves_result() -> color_eyre::Result<()> {
        use crate::message::{ResolveValue, ScriptCmd};
        use crate::proc::SpawnResult;
        let (runtime, sender, mut cmd_rx, mut push_rx) = spawn_with_cmds(
            r#"
            local h = mineral.spawn({"echo", "hi"}, { cwd = "/tmp", env = { K = "V" } },
                function(res, err)
                    mineral.ui.toast(res.code .. "/" .. res.stdout .. "/" .. tostring(res.killed))
                end)
            h:kill()
            "#,
        )?;
        let first = cmd_rx.try_recv()?;
        let ScriptCmd::Spawn { id, spec, query } = first else {
            color_eyre::eyre::bail!("期望 Spawn,实得 {first:?}");
        };
        assert_eq!(spec.program(), "echo");
        let kill = cmd_rx.try_recv()?;
        let ScriptCmd::SpawnKill { id: kill_id } = kill else {
            color_eyre::eyre::bail!("期望 SpawnKill,实得 {kill:?}");
        };
        assert_eq!(kill_id, id, "kill 必须路由到同一 spawn");
        // 模拟 daemon 泵回投结果
        sender.resolve(
            query,
            ResolveValue::Spawn(SpawnResult {
                code: Some(0),
                stdout: "hi\n".to_owned(),
                stderr: String::new(),
                killed: false,
            }),
        );
        let events = drain_after_stop(runtime, &mut push_rx);
        assert_eq!(
            events,
            vec![Event::Toast {
                kind: ToastKind::Info,
                content: vec![TextSpan::plain("0/hi\n/false")],
                id: None,
                ttl_secs: None,
            }],
            "spawn 回调收结构化结果 table"
        );
        Ok(())
    }

    #[test]
    fn spawn_empty_args_is_script_error() -> color_eyre::Result<()> {
        let (cmd_tx, _cmd_rx) = unbounded_channel();
        let (push_tx, _push_rx) = unbounded_channel();
        let host = crate::host::ScriptHost::new(cmd_tx, push_tx);
        let lua = Lua::new();
        install_api(&lua, &host)?;
        let result = lua.load(r#"mineral.spawn({}, function() end)"#).exec();
        assert!(result.is_err(), "空 args 必须当场报错");
        Ok(())
    }

    #[test]
    fn emit_loops_back_and_pushes_bus_event() -> color_eyre::Result<()> {
        use mineral_protocol::BusValue;
        let (runtime, _sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.on_message("my.ping", function(p)
                mineral.ui.toast(p.tag .. "/" .. p.list[2])
            end)
            mineral.emit("my.ping", { tag = "钠", list = { 10, 20 } })
            mineral.emit("no.subscriber", 42)
            "#,
        )?;
        let mut events = drain_after_stop(runtime, &mut push_rx);
        // Lua table 的 pairs 不保序:Map 载荷排序后再比对(集合等价)。
        for event in &mut events {
            if let Event::BusMessage {
                payload: BusValue::Map(entries),
                ..
            } = event
            {
                entries.sort_by(|a, b| a.0.cmp(&b.0));
            }
        }
        assert_eq!(
            events,
            vec![
                // emit 先下推 client(结构化载荷),再同步自环(toast 是自环副产物,
                // 但 toast 走同一 push 通道,顺序在 BusMessage 之后)。
                Event::BusMessage {
                    name: "my.ping".to_owned(),
                    payload: BusValue::Map(vec![
                        (
                            "list".to_owned(),
                            BusValue::Array(vec![BusValue::Int(10), BusValue::Int(20)]),
                        ),
                        ("tag".to_owned(), BusValue::Str("钠".to_owned())),
                    ]),
                },
                Event::Toast {
                    kind: ToastKind::Info,
                    content: vec![TextSpan::plain("钠/20")],
                    id: None,
                    ttl_secs: None,
                },
                Event::BusMessage {
                    name: "no.subscriber".to_owned(),
                    payload: BusValue::Int(42),
                },
            ],
            "emit 双路:下推结构化 BusMessage + 本 VM 同步自环"
        );
        Ok(())
    }

    #[test]
    fn emit_subscriber_error_does_not_bubble_to_emitter() -> color_eyre::Result<()> {
        let (runtime, _sender, mut push_rx) = spawn_with_script(
            r#"
            mineral.on_message("my.bad", function(p) error("炸") end)
            mineral.on_message("my.bad", function(p) mineral.ui.toast("第二个照常") end)
            mineral.emit("my.bad")
            mineral.ui.toast("emit 之后还活着")
            "#,
        )?;
        let events = drain_after_stop(runtime, &mut push_rx);
        let toasts = events
            .iter()
            .filter_map(|e| match e {
                Event::Toast { content, .. } => Some(flat(content)),
                _ => None,
            })
            .collect::<Vec<String>>();
        assert!(
            toasts.iter().any(|t| t == "第二个照常")
                && toasts.iter().any(|t| t == "emit 之后还活着"),
            "订阅者出错不影响其余订阅者与 emit 调用方,实得 {toasts:?}"
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
                content: vec![TextSpan::plain("dinged")],
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

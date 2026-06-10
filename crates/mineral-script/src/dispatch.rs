//! 脚本线程主循环:消费 [`ScriptMsg`],把事件分发给已注册的 Lua 回调。
//!
//! 回调经看门狗([`call_guarded`])保护;单个回调失败不影响同事件的
//! 其余回调 —— 记 `chain` 日志并向 client 推一条 Error toast(同
//! [`SCRIPT_ERROR_TOAST_ID`] 顶替,失败连发不刷屏)。

use std::sync::Arc;

use mineral_model::Song;
use mineral_protocol::{Event, TextSpan, ToastKind};
use mlua::Lua;

use crate::host::ScriptHost;
use crate::message::{ScriptEvent, ScriptMsg};
use crate::watchdog::{WatchdogConfig, call_guarded};

/// 脚本错误 toast 的顶替键:连续失败替换内容续命,不在 client 端堆叠刷屏。
const SCRIPT_ERROR_TOAST_ID: &str = "script.error";

/// 脚本线程入口:消费消息直到 [`ScriptMsg::Stop`] 或发送端全部关闭。
///
/// 等待方式按定时器状态自适应:无运行中定时器长等消息(零空转);有则
/// `recv_timeout` 到最近到期点,醒来收割到期回调 —— timer 心跳与消息
/// 处理共用一条线程,回调天然串行。
///
/// # Params:
///   - `lua`: 已 eval 过用户脚本的 VM(随线程独占)
///   - `host`: 宿主句柄(注册表 + 出方向通道)
///   - `watchdog`: 回调看门狗参数
///   - `rx`: 消息入口
pub(crate) fn run_loop(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    rx: &std::sync::mpsc::Receiver<ScriptMsg>,
) {
    use std::sync::mpsc::RecvTimeoutError;
    loop {
        let msg = match host.timers.lock().next_deadline() {
            None => rx
                .recv()
                .map_err(|_disconnected| RecvTimeoutError::Disconnected),
            Some(deadline) => {
                let wait = deadline.saturating_duration_since(std::time::Instant::now());
                rx.recv_timeout(wait)
            }
        };
        match msg {
            Ok(ScriptMsg::Event(event)) => dispatch_event(lua, host, watchdog, event),
            Ok(ScriptMsg::Action { name, ctx, reply }) => {
                // 回执接收端 drop(调用方超时放弃)时静默丢。
                let _ = reply.send(invoke_action(lua, host, watchdog, &name, ctx.as_ref()));
            }
            Ok(ScriptMsg::Resolve { query, value }) => {
                resolve_query(lua, host, watchdog, query, &value);
            }
            Ok(ScriptMsg::GetBinds { reply }) => {
                let _ = reply.send(host.events.lock().binds.clone());
            }
            Ok(ScriptMsg::Intercept { kind, ctx, reply }) => {
                // 回执接收端 drop(daemon 侧超时放弃)时静默丢。
                let _ = reply.send(run_hooks(lua, host, watchdog, kind, &ctx));
            }
            Ok(ScriptMsg::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        fire_due_timers(lua, host, watchdog);
    }
}

/// 收割并执行到期定时器回调(锁内收割改表,锁外调用)。
///
/// 消息处理后也跑一次:消息流不断时 `recv_timeout` 总是提前返回,
/// 不补这一刀定时器会被持续到达的消息饿死。
fn fire_due_timers(lua: &Lua, host: &ScriptHost, watchdog: &WatchdogConfig) {
    let due = host.timers.lock().collect_due(std::time::Instant::now());
    for key in due {
        let result = lua
            .registry_value::<mlua::Function>(&key)
            .and_then(|func| call_guarded::<_, ()>(lua, watchdog, &func, ()));
        if let Err(e) = result {
            report_callback_failure(host, "timer", &e);
        }
    }
}

/// 调用一个具名动作:查注册表(锁内取 Arc、锁外调)。回调收单一 ctx table
/// (无上下文触发面 = 空表;字段 nil 与缺字段在 Lua 侧无差别,加字段零破坏)。
///
/// 失败不推 error toast —— 结果经回执返回,由触发方(client)自行提示,
/// 避免双重提示。
fn invoke_action(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    name: &str,
    ctx: Option<&mineral_protocol::KeyContext>,
) -> crate::message::ActionOutcome {
    use crate::message::ActionOutcome;
    let Some(key) = host.events.lock().actions.get(name).cloned() else {
        return ActionOutcome::NotFound;
    };
    let result = ctx_table(lua, ctx).and_then(|ctx| {
        let func = lua.registry_value::<mlua::Function>(&key)?;
        call_guarded::<_, ()>(lua, watchdog, &func, ctx)
    });
    match result {
        Ok(()) => ActionOutcome::Done,
        Err(e) => {
            mineral_log::error!(
                target: "script",
                action = name,
                error = mineral_log::chain(&e),
                "script action failed"
            );
            // 回执只给首行(toast / CLI stderr 的人读信息);
            // mlua 错误的 traceback 多行,完整链已进上面的日志。
            let first_line = mineral_log::chain(&e)
                .lines()
                .next()
                .unwrap_or("脚本错误(详见日志)")
                .to_owned();
            ActionOutcome::Failed(first_line)
        }
    }
}

/// 回投一次异步查询的结果:pending 表取出回调(一次性),实参是
/// `(value, err)` 风格 —— 成功 `(值, nil)`,失败 `(nil, 错误串)`。
///
/// 锁内只取出回调,锁外构造实参并调用(回调里再发查询不撞锁)。
fn resolve_query(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    query: crate::QueryId,
    value: &crate::message::ResolveValue,
) {
    use crate::message::ResolveValue;
    let Some(key) = host.pending.lock().take(query) else {
        // 重复回投 / 线程重启后的迟到结果:静默丢。
        return;
    };
    let result = (|| -> mlua::Result<()> {
        let args: (mlua::Value, mlua::Value) = match value {
            ResolveValue::Store(v) => (crate::api::value::store_to_lua(lua, v)?, mlua::Value::Nil),
            ResolveValue::Songs(songs) => {
                let list = lua.create_table()?;
                for (i, song) in songs.iter().enumerate() {
                    list.set(i.wrapping_add(1), song_table(lua, song)?)?;
                }
                (mlua::Value::Table(list), mlua::Value::Nil)
            }
            ResolveValue::Playlists(playlists) => {
                let list = lua.create_table()?;
                for (i, p) in playlists.iter().enumerate() {
                    let entry = lua.create_table()?;
                    entry.set("id", p.id.qualified())?;
                    entry.set("name", p.name.clone())?;
                    entry.set("track_count", p.track_count)?;
                    list.set(i.wrapping_add(1), entry)?;
                }
                (mlua::Value::Table(list), mlua::Value::Nil)
            }
            ResolveValue::Spawn(result) => {
                let entry = lua.create_table()?;
                // 被信号终止(含 kill)无退出码:字段缺席,Lua 读出 nil。
                if let Some(code) = result.code {
                    entry.set("code", code)?;
                }
                entry.set("stdout", result.stdout.clone())?;
                entry.set("stderr", result.stderr.clone())?;
                entry.set("killed", result.killed)?;
                (mlua::Value::Table(entry), mlua::Value::Nil)
            }
            ResolveValue::Error(msg) => (
                mlua::Value::Nil,
                mlua::Value::String(lua.create_string(msg)?),
            ),
        };
        let func = lua.registry_value::<mlua::Function>(&key)?;
        call_guarded::<_, ()>(lua, watchdog, &func, args)
    })();
    if let Err(e) = result {
        report_callback_failure(host, "query", &e);
    }
}

/// 把一个事件分发给对应桶里的全部回调(注册顺序)。
///
/// 回调统一收**单一 args table**(nvim autocmd 风格):以后给事件加字段
/// 零破坏,LSP 侧由 meta stub 的 per-event `@class` + `@overload` 给强类型。
fn dispatch_event(lua: &Lua, host: &ScriptHost, watchdog: &WatchdogConfig, event: ScriptEvent) {
    match event {
        ScriptEvent::TrackStarted { song } => {
            let callbacks = host.events.lock().track_started.clone();
            invoke_all(lua, host, watchdog, &callbacks, "track_started", |lua| {
                let args = lua.create_table()?;
                args.set("song", song_table(lua, &song)?)?;
                Ok(args)
            });
        }
        ScriptEvent::TrackFinished { song, reason } => {
            // 锁内只克隆 Arc 列表,锁外调回调 —— 回调里再 `mineral.on` 不死锁。
            let callbacks = host.events.lock().track_finished.clone();
            invoke_all(lua, host, watchdog, &callbacks, "track_finished", |lua| {
                let args = lua.create_table()?;
                args.set("song", song_table(lua, &song)?)?;
                args.set("reason", reason.as_str())?;
                Ok(args)
            });
        }
        ScriptEvent::DownloadCompleted {
            song,
            path,
            quality,
            format,
        } => {
            let callbacks = host.events.lock().download_completed.clone();
            invoke_all(
                lua,
                host,
                watchdog,
                &callbacks,
                "download_completed",
                |lua| {
                    let args = lua.create_table()?;
                    args.set("song", song_table(lua, &song)?)?;
                    args.set("path", path.display().to_string())?;
                    args.set("quality", quality.as_str())?;
                    // 拿不到格式(Other(""))→ 缺席为 nil,不给空串。
                    let fmt = format.as_str();
                    args.set("format", (!fmt.is_empty()).then(|| fmt.to_owned()))?;
                    Ok(args)
                },
            );
        }
        ScriptEvent::PropertyChanged { key, value } => {
            // 同一锁内更新缓存 + 快照观察者:后注册的 observe 回放到的
            // 一定是本次或更新的值,不会读到旧值。
            let callbacks = {
                let mut registry = host.events.lock();
                registry.props.insert(key, value.clone());
                registry.observers.get(&key).cloned().unwrap_or_default()
            };
            invoke_all(lua, host, watchdog, &callbacks, key.as_str(), |lua| {
                crate::api::value::prop_to_lua(lua, &value)
            });
        }
    }
}

/// 跑一类同步拦截 hook:按注册顺序调用,首个非放行裁决短路生效。
///
/// 回调收 ctx table(`song` / `url` / `quality` / `kind`),返回值解释:
/// `nil`(或 `true`)= 放行;`false` / `{ skip = 原因 }` = 跳过;
/// `{ url = ?, quality = ? }` = 改写。Lua 错误 / 非法返回值按放行处理
/// (拦截失败不致命),记日志 + error toast。
fn run_hooks(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    kind: crate::hooks::HookKind,
    ctx: &crate::hooks::HookContext,
) -> crate::hooks::HookDecision {
    use crate::hooks::HookDecision;
    // 锁内只克隆 Arc 列表,锁外调回调(回调里再注册不撞锁)。
    let callbacks = host
        .events
        .lock()
        .hooks
        .get(&kind)
        .cloned()
        .unwrap_or_default();
    for key in &callbacks {
        let outcome = hook_ctx_table(lua, kind, ctx).and_then(|args| {
            let func = lua.registry_value::<mlua::Function>(key)?;
            call_guarded::<_, mlua::Value>(lua, watchdog, &func, args)
        });
        match outcome {
            Ok(value) => match interpret_hook_return(&value) {
                Ok(HookDecision::Continue) => {}
                Ok(decision) => return decision,
                Err(msg) => {
                    report_callback_failure(host, kind.as_str(), &mlua::Error::runtime(msg));
                }
            },
            Err(e) => report_callback_failure(host, kind.as_str(), &e),
        }
    }
    HookDecision::Continue
}

/// 拦截回调的 ctx table:`song`(最小投影)+ `url`(字符串)+
/// `quality`(音质名)+ `kind`(hook 名)。
fn hook_ctx_table(
    lua: &Lua,
    kind: crate::hooks::HookKind,
    ctx: &crate::hooks::HookContext,
) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.set("song", song_table(lua, ctx.song())?)?;
    table.set("url", ctx.original().url.to_string())?;
    table.set("quality", ctx.original().quality.as_str())?;
    table.set("kind", kind.as_str())?;
    Ok(table)
}

/// 把 hook 回调的 Lua 返回值解释成裁决;非法形态报 `Err`(按放行处理)。
fn interpret_hook_return(value: &mlua::Value) -> Result<crate::hooks::HookDecision, String> {
    use crate::hooks::{HookDecision, RewriteSpec};
    match value {
        mlua::Value::Nil | mlua::Value::Boolean(true) => Ok(HookDecision::Continue),
        mlua::Value::Boolean(false) => Ok(HookDecision::Skip {
            reason: "脚本跳过".to_owned(),
        }),
        mlua::Value::Table(table) => {
            if let Some(reason) = table
                .get::<Option<String>>("skip")
                .map_err(|e| format!("hook 返回值 skip 字段非法: {e}"))?
            {
                return Ok(HookDecision::Skip { reason });
            }
            let new_url = table
                .get::<Option<String>>("url")
                .map_err(|e| format!("hook 返回值 url 字段非法: {e}"))?
                .map(|raw| {
                    raw.parse::<mineral_model::MediaUrl>()
                        .map_err(|e| format!("hook 返回的 url 解析失败: {e}"))
                })
                .transpose()?;
            let new_quality = table
                .get::<Option<String>>("quality")
                .map_err(|e| format!("hook 返回值 quality 字段非法: {e}"))?
                .map(|raw| parse_bitrate(&raw))
                .transpose()?;
            if new_url.is_none() && new_quality.is_none() {
                return Err("hook 返回 table 但无 url / quality / skip 字段".to_owned());
            }
            Ok(HookDecision::Rewrite(RewriteSpec {
                new_url,
                new_quality,
            }))
        }
        other => Err(format!(
            "hook 返回值须是 nil / boolean / table,实得 {}",
            other.type_name()
        )),
    }
}

/// 按音质名解析 [`mineral_model::BitRate`](与 `as_str` 对偶);未知名报错。
fn parse_bitrate(raw: &str) -> Result<mineral_model::BitRate, String> {
    mineral_model::BitRate::ALL
        .into_iter()
        .find(|q| q.as_str() == raw)
        .ok_or_else(|| format!("未知音质名 `{raw}`(可选:standard/higher/exhigh/lossless/hires)"))
}

/// 依次调用一桶回调;实参由 `make_args` 现做(每个回调独立一份,互不污染)。
///
/// # Params:
///   - `callbacks`: 锁外快照的回调键列表
///   - `event_name`: 事件名(日志 / toast 文案用)
///   - `make_args`: 构造本次调用实参(失败按回调失败同等处理)
fn invoke_all<A: mlua::IntoLuaMulti>(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    callbacks: &[Arc<mlua::RegistryKey>],
    event_name: &str,
    make_args: impl Fn(&Lua) -> mlua::Result<A>,
) {
    for key in callbacks {
        let result = make_args(lua).and_then(|args| {
            let func = lua.registry_value::<mlua::Function>(key)?;
            call_guarded::<_, ()>(lua, watchdog, &func, args)
        });
        if let Err(e) = result {
            report_callback_failure(host, event_name, &e);
        }
    }
}

/// 回调失败的统一出口:完整链进日志,提示进 client toast。
/// (`emit` 自环调订阅者也走这里,故 `pub(crate)`。)
pub(crate) fn report_callback_failure(host: &ScriptHost, event_name: &str, e: &mlua::Error) {
    mineral_log::error!(
        target: "script",
        event = event_name,
        error = mineral_log::chain(e),
        "script callback failed"
    );
    let _ = host.push.send(Event::Toast {
        kind: ToastKind::Error,
        content: vec![TextSpan::plain(format!(
            "脚本 {event_name} 回调出错,详见日志"
        ))],
        id: Some(SCRIPT_ERROR_TOAST_ID.to_owned()),
        ttl_secs: None,
    });
}

/// 按键上下文在 Lua 侧的投影:蛇形字段名,缺席字段不设(Lua 读出 nil)。
///
/// `view` 用 [`mineral_protocol::ViewKind::script_name`] 蛇形名;歌投影成
/// [`song_table`](`{id, title, duration_ms}`,id 可直接回喂 player / store API),
/// 歌单投影成 `{id, name}`。
fn ctx_table(lua: &Lua, ctx: Option<&mineral_protocol::KeyContext>) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    let Some(ctx) = ctx else {
        return Ok(table);
    };
    table.set("view", ctx.view().script_name())?;
    if let Some(song) = ctx.selected_song() {
        table.set("selected_song", song_table(lua, song)?)?;
    }
    if let Some(playlist) = ctx.selected_playlist() {
        let entry = lua.create_table()?;
        entry.set("id", playlist.id.qualified())?;
        entry.set("name", playlist.name.clone())?;
        table.set("selected_playlist", entry)?;
    }
    if let Some(song) = ctx.now_playing() {
        table.set("now_playing", song_table(lua, song)?)?;
    }
    if let Some(loved) = ctx.selected_loved() {
        table.set("selected_loved", *loved)?;
    }
    if let Some(query) = ctx.search_query() {
        table.set("search_query", query.clone())?;
    }
    Ok(table)
}

/// `Song` 在 Lua 侧的投影
///
/// id 用 `qualified()`(全局唯一,可直接回喂 `mineral.player.play`);
fn song_table(lua: &Lua, song: &Song) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.set("id", song.id.qualified())?;
    table.set("title", song.name.clone())?;
    table.set("duration_ms", song.duration_ms)?;
    table.set(
        "artists",
        lua.create_sequence_from(song.artists.iter().map(|a| a.name.clone()))?,
    )?;
    // Option 字段拿不到 → 缺席(Lua 侧读出 nil)。
    table.set("album", song.album.as_ref().map(|a| a.name.clone()))?;
    // MediaUrl 统一投影成字符串:远端 = http(s) URL,本地 = 绝对路径。
    table.set(
        "cover_url",
        song.cover_url.as_ref().map(ToString::to_string),
    )?;
    table.set(
        "source_url",
        song.source_url.as_ref().map(ToString::to_string),
    )?;
    Ok(table)
}

//! 脚本线程主循环:消费 [`ScriptMsg`],把事件分发给已注册的 Lua 回调。
//!
//! 回调经看门狗([`call_guarded`])保护;单个回调失败不影响同事件的
//! 其余回调 —— 记 `chain` 日志并向 client 推一条 Error toast(同
//! [`SCRIPT_ERROR_TOAST_ID`] 顶替,失败连发不刷屏)。

use std::sync::Arc;

use mineral_model::Song;
use mineral_protocol::{Event, ToastKind};
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
            ActionOutcome::Failed(mineral_log::chain(&e))
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
        ScriptEvent::DownloadCompleted { song, path } => {
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
fn report_callback_failure(host: &ScriptHost, event_name: &str, e: &mlua::Error) {
    mineral_log::error!(
        target: "script",
        event = event_name,
        error = mineral_log::chain(e),
        "script callback failed"
    );
    let _ = host.push.send(Event::Toast {
        kind: ToastKind::Error,
        content: format!("脚本 {event_name} 回调出错,详见日志"),
        id: Some(SCRIPT_ERROR_TOAST_ID.to_owned()),
        ttl_secs: None,
    });
}

/// 按键上下文在 Lua 侧的投影:蛇形字段名,缺席字段不设(Lua 读出 nil)。
///
/// `view` 用 [`mineral_protocol::ViewKind::script_name`] 蛇形名;id 一律
/// `qualified()` 字符串,可直接回喂 `mineral.player.play` / `mineral.store.*`。
fn ctx_table(lua: &Lua, ctx: Option<&mineral_protocol::KeyContext>) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    let Some(ctx) = ctx else {
        return Ok(table);
    };
    table.set("view", ctx.view().script_name())?;
    if let Some(id) = ctx.selected_song_id() {
        table.set("selected_song_id", id.qualified())?;
    }
    if let Some(id) = ctx.selected_playlist_id() {
        table.set("selected_playlist_id", id.qualified())?;
    }
    if let Some(id) = ctx.now_playing_id() {
        table.set("now_playing_id", id.qualified())?;
    }
    Ok(table)
}

/// `Song` 在 Lua 侧的最小投影:`{ id, title, duration_ms }`。
///
/// id 用 `qualified()`(全局唯一,可直接回喂 `mineral.player.play`);
/// 完整字段集(artists / album / cover)的归一化投影是 TODO(sub05)。
fn song_table(lua: &Lua, song: &Song) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.set("id", song.id.qualified())?;
    table.set("title", song.name.clone())?;
    table.set("duration_ms", song.duration_ms)?;
    Ok(table)
}

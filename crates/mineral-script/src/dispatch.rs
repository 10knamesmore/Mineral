//! 脚本线程主循环:消费 [`ScriptMsg`],把事件分发给已注册的 Lua 回调。
//!
//! 回调经看门狗([`call_guarded`])保护;单个回调失败不影响同事件的
//! 其余回调 —— 记 `chain` 日志并向 client 推一条 Error toast(同
//! [`SCRIPT_ERROR_TOAST_ID`] 顶替,失败连发不刷屏)。

use std::sync::Arc;

use mineral_model::Song;
use mineral_protocol::{Event, ToastKind};
use mlua::Lua;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::host::ScriptHost;
use crate::message::{ScriptEvent, ScriptMsg};
use crate::watchdog::{WatchdogConfig, call_guarded};

/// 脚本错误 toast 的顶替键:连续失败替换内容续命,不在 client 端堆叠刷屏。
const SCRIPT_ERROR_TOAST_ID: &str = "script.error";

/// 脚本线程入口:阻塞消费消息直到 [`ScriptMsg::Stop`] 或发送端全部关闭。
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
    mut rx: UnboundedReceiver<ScriptMsg>,
) {
    while let Some(msg) = rx.blocking_recv() {
        match msg {
            ScriptMsg::Event(event) => dispatch_event(lua, host, watchdog, event),
            ScriptMsg::Action { name, reply } => {
                // 回执接收端 drop(调用方超时放弃)时静默丢。
                let _ = reply.send(invoke_action(lua, host, watchdog, &name));
            }
            ScriptMsg::Stop => break,
        }
    }
}

/// 调用一个具名动作:查注册表(锁内取 Arc、锁外调),ctx 为空表
/// (单 args table 风格,以后加字段零破坏)。
///
/// 失败不推 error toast —— 结果经回执返回,由触发方(client)自行提示,
/// 避免双重提示。
fn invoke_action(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    name: &str,
) -> crate::message::ActionOutcome {
    use crate::message::ActionOutcome;
    let Some(key) = host.events.lock().actions.get(name).cloned() else {
        return ActionOutcome::NotFound;
    };
    let result = lua.create_table().and_then(|ctx| {
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
    });
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

//! 在脚本线程中串行处理消息与到期定时器。

use mlua::Lua;

use super::callbacks::{dispatch_event, invoke_action, report_callback_failure, resolve_query};
use super::transforms::{
    curate_source_keys, render_copy_template, run_curate, run_queue_transform,
};
use crate::host::ScriptHost;
use crate::message::ScriptMsg;
use crate::watchdog::{WatchdogConfig, call_guarded};

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
            Ok(ScriptMsg::Action {
                name,
                ctx,
                args,
                reply,
            }) => {
                // 回执接收端 drop(调用方超时放弃)时静默丢。
                let _ = reply.send(invoke_action(
                    lua,
                    host,
                    watchdog,
                    &name,
                    ctx.as_ref(),
                    &args,
                ));
            }
            Ok(ScriptMsg::Resolve { query, value }) => {
                resolve_query(lua, host, watchdog, query, &value);
            }
            Ok(ScriptMsg::GetBinds { reply }) => {
                let _ = reply.send(host.events.lock().binds.clone());
            }
            Ok(ScriptMsg::InterceptStream { ctx, reply }) => {
                crate::intercept::run_stream(lua, host, watchdog, &ctx, reply);
            }
            Ok(ScriptMsg::InterceptDownload { ctx, reply }) => {
                crate::intercept::run_download(lua, host, watchdog, &ctx, reply);
            }
            Ok(ScriptMsg::CuratePlaylists {
                source,
                briefs,
                reply,
            }) => {
                // 回执接收端 drop(daemon 侧超时放弃)时静默丢。
                let _ = reply.send(run_curate(lua, host, watchdog, source.as_ref(), &briefs));
            }
            Ok(ScriptMsg::GetCurateKeys { reply }) => {
                let _ = reply.send(curate_source_keys(lua));
            }
            Ok(ScriptMsg::RenderCopyTemplate { index, ctx, reply }) => {
                let _ = reply.send(render_copy_template(lua, watchdog, index, &ctx));
            }
            Ok(ScriptMsg::QueueTransform {
                index,
                queue,
                current,
                selected,
                reply,
            }) => {
                let _ = reply.send(run_queue_transform(
                    lua, watchdog, index, &queue, current, selected,
                ));
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

//! 在 daemon 脚本线程中串行执行音乐回调与配置函数。

use mlua::Lua;

use super::callbacks::resolve_query;
use super::transforms::{curate_source_keys, run_curate, run_queue_transform};
use crate::host::ScriptHost;
use crate::message::ScriptMsg;
use crate::watchdog::WatchdogConfig;

/// 消费消息直到停机或全部发送端关闭；没有定时器或轮询心跳。
///
/// # Params:
///   - `lua`: 已求值的 daemon VM，线程独占
///   - `host`: hook 注册表、在途查询与命令出口
///   - `watchdog`: 回调的看门狗参数
///   - `rx`: 同步消息入口
pub(crate) fn run_loop(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    rx: &std::sync::mpsc::Receiver<ScriptMsg>,
) {
    while let Ok(msg) = rx.recv() {
        match msg {
            ScriptMsg::Resolve { query, value } => {
                resolve_query(lua, host, watchdog, query, &value);
            }
            ScriptMsg::InterceptStream { ctx, reply } => {
                crate::intercept::run_stream(lua, host, watchdog, &ctx, reply);
            }
            ScriptMsg::InterceptDownload { ctx, reply } => {
                crate::intercept::run_download(lua, host, watchdog, &ctx, reply);
            }
            ScriptMsg::CuratePlaylists {
                source,
                briefs,
                reply,
            } => {
                let _ = reply.send(run_curate(lua, host, watchdog, source.as_ref(), &briefs));
            }
            ScriptMsg::GetCurateKeys { reply } => {
                let _ = reply.send(curate_source_keys(lua));
            }
            ScriptMsg::QueueTransform {
                name,
                queue,
                current,
                selected,
                reply,
            } => {
                let _ = reply.send(run_queue_transform(
                    lua, watchdog, &name, &queue, current, selected,
                ));
            }
            ScriptMsg::Stop => break,
        }
    }
}

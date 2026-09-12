//! 执行已注册的动作、事件与异步查询回调。
//!
//! 单个回调失败不影响同事件的其余回调；完整错误链写入日志，
//! 错误提示使用固定顶替键，连续失败不堆叠提示。

use std::sync::Arc;

use mineral_protocol::{Event, TextSpan, ToastKind};
use mlua::Lua;

use super::projection::{briefs_table, direct_media_table, playlist_entry_table, song_table};
use crate::host::ScriptHost;
use crate::message::ScriptEvent;
use crate::watchdog::{WatchdogConfig, call_guarded};

/// 脚本错误 toast 的顶替键:连续失败替换内容续命,不在 client 端堆叠刷屏。
const SCRIPT_ERROR_TOAST_ID: &str = "script.error";

/// 调用一个具名动作:查注册表(锁内取 Arc、锁外调)。回调收单一 ctx table
/// (无上下文触发面 = 空表;字段 nil 与缺字段在 Lua 侧无差别,加字段零破坏)。
///
/// 失败不推 error toast —— 结果经回执返回,由触发方(client)自行提示,
/// 避免双重提示。
pub(super) fn invoke_action(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    name: &str,
    ctx: Option<&mineral_protocol::KeyContext>,
    args: &[String],
) -> crate::message::ActionOutcome {
    use crate::message::ActionOutcome;
    let Some(key) = host.events.lock().actions.get(name).cloned() else {
        return ActionOutcome::NotFound;
    };
    let result = ctx_table(lua, ctx, args).and_then(|ctx| {
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
pub(super) fn resolve_query(
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
            ResolveValue::PlaylistEntries(entries) => {
                let list = lua.create_table()?;
                for (i, entry) in entries.iter().enumerate() {
                    list.set(i.wrapping_add(1), playlist_entry_table(lua, entry)?)?;
                }
                (mlua::Value::Table(list), mlua::Value::Nil)
            }
            ResolveValue::Playlists(playlists) => (
                mlua::Value::Table(briefs_table(lua, playlists)?),
                mlua::Value::Nil,
            ),
            ResolveValue::DirectMedia(direct) => (
                mlua::Value::Table(direct_media_table(lua, direct)?),
                mlua::Value::Nil,
            ),
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
/// 回调统一收**单一 args table**(nvim autocmd 风格),使事件字段集中在一个载荷中;
/// LSP 侧由 meta stub 的 per-event `@class` + `@overload` 提供强类型。
pub(super) fn dispatch_event(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    event: ScriptEvent,
) {
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
                    // 拿不到格式 → 缺席为 nil,不给空串。
                    args.set("format", format.as_ref().map(|f| f.as_str().to_owned()))?;
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
fn ctx_table(
    lua: &Lua,
    ctx: Option<&mineral_protocol::KeyContext>,
    args: &[String],
) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    // args 恒设为数组:CLI 触发带位置实参,TUI 键位 / 无参 CLI 为空数组(非 nil)。
    table.set("args", lua.create_sequence_from(args.iter().cloned())?)?;
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

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::invoke_action;
    use crate::api::test_support::vm_with_host;
    use crate::message::ActionOutcome;
    use crate::watchdog::WatchdogConfig;

    /// 宽松看门狗:测试里不期望被超时中断。
    fn loose_watchdog() -> WatchdogConfig {
        WatchdogConfig::builder()
            .instruction_interval(100_000)
            .soft_wall(Duration::from_secs(5))
            .hard_wall(Duration::from_secs(10))
            .build()
    }

    /// CLI 触发(ctx=None、带位置实参)时,args 数组经 `ctx.args` 送达回调。
    /// 断言写在 Lua 侧:任一不符则回调报错 → `ActionOutcome::Failed`,测试失败带原因。
    #[test]
    fn action_receives_cli_args() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        lua.load(
            r#"
            mineral.action("t.args", function(ctx)
                assert(type(ctx.args) == "table", "args 必须是 table")
                assert(#ctx.args == 2, "args 长度必须是 2")
                assert(ctx.args[1] == "hello", "args[1] 必须是 hello")
                assert(ctx.args[2] == "world", "args[2] 必须是 world")
            end)
            "#,
        )
        .exec()?;
        let outcome = invoke_action(
            &lua,
            &host,
            &loose_watchdog(),
            "t.args",
            /*ctx*/ None,
            &["hello".to_owned(), "world".to_owned()],
        );
        assert!(
            matches!(outcome, ActionOutcome::Done),
            "带 args 的动作应成功执行,实际 {outcome:?}"
        );
        Ok(())
    }

    /// 无 args(TUI 键位 / 无参 CLI)时 `ctx.args` 是空数组(长度 0,非 nil)。
    #[test]
    fn action_args_default_empty_array() -> color_eyre::Result<()> {
        let (lua, host) = vm_with_host()?;
        lua.load(
            r#"
            mineral.action("t.noargs", function(ctx)
                assert(type(ctx.args) == "table", "args 必须是 table")
                assert(#ctx.args == 0, "无实参时 args 必须是空数组")
            end)
            "#,
        )
        .exec()?;
        let outcome = invoke_action(
            &lua,
            &host,
            &loose_watchdog(),
            "t.noargs",
            /*ctx*/ None,
            &[],
        );
        assert!(
            matches!(outcome, ActionOutcome::Done),
            "无 args 的动作应成功执行,实际 {outcome:?}"
        );
        Ok(())
    }
}

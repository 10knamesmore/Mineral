//! 音乐查询完成时执行一次回调，失败类别经 daemon 推送给 client。

use mineral_protocol::{Event, FailureNotice};
use mlua::Lua;

use crate::host::ScriptHost;
use crate::projection::{briefs_table, direct_media_table, playlist_entry_table, song_table};
use crate::watchdog::{WatchdogConfig, call_guarded};

/// 取出在途查询回调并执行一次；成功收 `(value, nil)`，失败收 `(nil, err)`。
/// 锁只用于取出回调，Lua 执行时不持锁，允许回调继续发音乐查询。
pub(super) fn resolve_query(
    lua: &Lua,
    host: &ScriptHost,
    watchdog: &WatchdogConfig,
    query: crate::QueryId,
    value: &crate::message::ResolveValue,
) {
    use crate::message::ResolveValue;
    let Some(key) = host.pending.lock().take(query) else {
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
            ResolveValue::DirectMedia {
                media,
                requested_quality,
            } => (
                mlua::Value::Table(direct_media_table(lua, media, *requested_quality)?),
                mlua::Value::Nil,
            ),
            ResolveValue::Error(error) => (
                mlua::Value::Nil,
                mlua::Value::String(lua.create_string(mineral_log::chain(error.as_ref()))?),
            ),
        };
        let func = lua.registry_value::<mlua::Function>(&key)?;
        call_guarded::<_, ()>(lua, watchdog, &func, args)
    })();
    if let Err(e) = result {
        report_callback_failure(host, "query", &e);
    }
}

/// 完整回调错误链写日志，只推送失败类别，展示文案由 client 生成。
pub(crate) fn report_callback_failure(host: &ScriptHost, callback_name: &str, e: &mlua::Error) {
    mineral_log::error!(
        target: "script",
        callback = callback_name,
        error = mineral_log::chain(e),
        "script callback failed"
    );
    let _ = host
        .push
        .send(Event::Failure(FailureNotice::ScriptCallbackFailed {
            callback: callback_name.to_owned(),
        }));
}

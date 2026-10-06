//! 摘取本宿主配置回调,留在同一 VM 中,不进入配置数据树。

use mineral_config::{extract_setup, table_path};
use mineral_script::mlua::{self, Function, Lua, LuaSerdeExt, Table, Value};
use mineral_script::registry::{
    CURATE_PLAYLISTS_MERGED_FN, CURATE_PLAYLISTS_SOURCE_FNS, DAEMON_SETUP_FN, QUEUE_TRANSFORM_FNS,
};

/// 标记 daemon 数组并摘取音乐函数与根 setup;不读取 TUI 字段。
pub(super) fn prepare_daemon(lua: &Lua, merged: &Table) -> mlua::Result<()> {
    if let Some(local) = table_path(merged, &["sources", "local"])
        && let Ok(roots) = local.get::<Table>("roots")
    {
        roots.set_metatable(Some(lua.array_metatable()));
    }
    extract_setup(lua, merged, DAEMON_SETUP_FN)?;
    extract_playlist_transforms(lua, merged)?;
    extract_queue_transforms(lua, merged)
}

/// 摘取 queue.transforms 中的函数,按名称存入 registry;名称唯一性由落型校验。
fn extract_queue_transforms(lua: &Lua, merged: &Table) -> mlua::Result<()> {
    let functions = lua.create_table()?;
    if let Some(transforms) = table_path(merged, &["queue", "transforms"]) {
        transforms.set_metatable(Some(lua.array_metatable()));
        for i in 1..=transforms.raw_len() {
            let Ok(item) = transforms.get::<Table>(i) else {
                continue;
            };
            let function = item.get::<Function>("transform")?;
            if let Ok(name) = item.get::<String>("name") {
                functions.raw_set(name, function)?;
            }
            item.raw_set("transform", Value::Nil)?;
        }
    }
    lua.set_named_registry_value(QUEUE_TRANSFORM_FNS, functions)
}

/// 摘取各源及跨源的歌单策展函数;没有数据字段的自定义来源不占 schema 段。
fn extract_playlist_transforms(lua: &Lua, merged: &Table) -> mlua::Result<()> {
    let functions = lua.create_table()?;
    let mut merged_function = Value::Nil;
    if let Some(sources) = table_path(merged, &["sources"]) {
        if let Ok(function) = sources.get::<Function>("curate_playlists") {
            merged_function = Value::Function(function);
            sources.raw_set("curate_playlists", Value::Nil)?;
        }
        // 修改迭代中的 sources 会跳过条目,所以先记录需要移除的空段。
        let mut emptied = Vec::<Value>::new();
        for pair in sources.pairs::<Value, Value>() {
            let (key, value) = pair?;
            let Value::Table(section) = value else {
                continue;
            };
            if let Ok(function) = section.get::<Function>("curate_playlists") {
                functions.raw_set(key.clone(), function)?;
                section.raw_set("curate_playlists", Value::Nil)?;
                if section.is_empty() {
                    emptied.push(key);
                }
            }
        }
        for key in emptied {
            sources.raw_set(key, Value::Nil)?;
        }
    }
    lua.set_named_registry_value(CURATE_PLAYLISTS_SOURCE_FNS, functions)?;
    lua.set_named_registry_value(CURATE_PLAYLISTS_MERGED_FN, merged_function)
}

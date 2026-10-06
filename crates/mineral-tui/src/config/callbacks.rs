//! 摘取本宿主配置回调,留在同一 VM 中,不进入配置数据树。

use mineral_config::{extract_setup, table_path};
use mineral_script::mlua::{self, Function, Lua, LuaSerdeExt, Table, Value};
use mineral_script::registry::{COPY_TEMPLATE_FNS, TUI_SETUP_FN};

/// 标记 TUI 数组并摘取本地复制模板与根 setup;不读取 daemon 字段。
pub(super) fn prepare_tui(lua: &Lua, merged: &Table) -> mlua::Result<()> {
    extract_setup(lua, merged, TUI_SETUP_FN)?;
    extract_copy_templates(lua, merged)
}

/// 摘取本地复制模板,按数组顺序存入 registry;空表须标记为数组以落成 Vec。
fn extract_copy_templates(lua: &Lua, merged: &Table) -> mlua::Result<()> {
    let functions = lua.create_table()?;
    if let Some(templates) = table_path(merged, &["copy", "templates"]) {
        templates.set_metatable(Some(lua.array_metatable()));
        for i in 1..=templates.raw_len() {
            let Ok(item) = templates.get::<Table>(i) else {
                continue;
            };
            let function = item.get::<Function>("template")?;
            functions.raw_set(i, function)?;
            item.raw_set("template", Value::Nil)?;
        }
    }
    lua.set_named_registry_value(COPY_TEMPLATE_FNS, functions)
}

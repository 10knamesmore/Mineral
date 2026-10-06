//! 将各自宿主表传给配置中提取的 setup，执行失败不提交宿主暂存命令。

use mlua::Lua;

use crate::api;
use crate::watchdog::{WatchdogConfig, call_guarded};

/// 执行可选 setup；调用方只在此函数与后续装配都成功后提交命令。
pub(crate) fn run(
    lua: &Lua,
    watchdog: &WatchdogConfig,
    registry: &str,
    module: &str,
) -> mlua::Result<()> {
    let Some(setup) = lua.named_registry_value::<Option<mlua::Function>>(registry)? else {
        return Ok(());
    };
    let host = api::module_table(lua, module)?;
    let result = call_guarded::<_, ()>(lua, watchdog, &setup, host);
    if let Err(source) = &result {
        mineral_log::error!(
            target: "script",
            module,
            error = mineral_log::chain(source),
            "script setup failed"
        );
    }
    result
}

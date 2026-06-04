//! 非 daemon 进程的 host API no-op stub 注入。
//!
//! 用户 `config.lua` 顶层可能调 `mineral.on(...)` / `mineral.player.toggle()` 等;
//! 在 TUI / CLI / 守卫测试这类无 daemon VM 的进程里,这些调用必须安全无副作用。
//! daemon 进程**不**调本函数,改由脚本运行时注入活实现(同名同形)。

use mlua::{Lua, MultiValue, Value};

/// 在 VM 注入 `mineral` 全局表及子表 `player`/`ui`/`log`,其中 `on`/`action`/`bind`/
/// `observe`/`get`/命令族/`toast`/`log.*` 均为 no-op(吞参、返 nil),保证用户
/// `config.lua` 顶层调用不报错。
///
/// 这是非 daemon 进程的注入点;daemon 脚本运行时跳过本函数、注入活实现。
///
/// # Params:
///   - `lua`: 目标 VM
///
/// # Return:
///   注入成功;表创建 / 赋值失败时返回 `Err`
pub fn inject_noop_host(lua: &Lua) -> color_eyre::Result<()> {
    let mineral = lua.create_table()?;

    // 顶层族:事件 / 动作 / 订阅 / 命令(全部吞参返 nil)。
    for name in ["on", "action", "bind", "observe", "get", "download"] {
        mineral.set(name, noop(lua)?)?;
    }

    let player = lua.create_table()?;
    for name in [
        "toggle",
        "next",
        "prev",
        "stop",
        "seek_rel",
        "seek_to",
        "set_volume",
        "set_mode",
        "play",
    ] {
        player.set(name, noop(lua)?)?;
    }
    mineral.set("player", player)?;

    let ui = lua.create_table()?;
    ui.set("toast", noop(lua)?)?;
    mineral.set("ui", ui)?;

    let log = lua.create_table()?;
    for name in ["info", "warn"] {
        log.set(name, noop(lua)?)?;
    }
    mineral.set("log", log)?;

    lua.globals().set("mineral", mineral)?;
    Ok(())
}

/// 造一个吞掉任意参数、返回 nil 的 Lua 函数。
///
/// # Params:
///   - `lua`: 目标 VM
///
/// # Return:
///   no-op Lua 函数
fn noop(lua: &Lua) -> color_eyre::Result<mlua::Function> {
    let f = lua.create_function(|_, _: MultiValue| Ok(Value::Nil))?;
    Ok(f)
}

//! 非 daemon 进程的 host API 吞噬桩注入。
//!
//! 用户 `config.lua` 顶层可能调 `mineral.on(...)` / `mineral.timer.every(...)` 等;
//! 在 TUI / CLI / 守卫测试这类无 daemon VM 的进程里,这些调用必须安全无副作用。
//! daemon 进程**不**调本函数,改由脚本运行时注入活实现。

use mlua::{Lua, MultiValue};

/// 在 VM 注入 `mineral` 全局——一张「吞噬表」:任意字段访问返回它自己、
/// 被当函数调用也返回它自己。`mineral.任何.链式(...):方法()` 全部静默,
/// daemon 加新 API 这里**永远不用同步**。
///
/// 拼错 API 名在本桩下同样静默——拼写检查归 daemon 真表(报错进日志)
/// 与 LSP meta stub(编辑器内补全 / 检查),不归这里。
///
/// # Params:
///   - `lua`: 目标 VM
///
/// # Return:
///   注入成功;表创建 / 赋值失败时返回 `Err`
pub fn inject_noop_host(lua: &Lua) -> color_eyre::Result<()> {
    let absorber = lua.create_table()?;
    let meta = lua.create_table()?;
    // 任意字段访问 → 吞噬表自身(支持任意深度的子表链)。
    // 必须用函数形式:表形式的 `__index = absorber` 是「查不到再去 absorber 查」,
    // absorber 里本来就没有,Lua 会沿链递归直到报 chain too long。
    let inner = absorber.clone();
    meta.set(
        "__index",
        lua.create_function(move |_, _: MultiValue| Ok(inner.clone()))?,
    )?;
    // 被当函数调用 → 吞参、返回吞噬表自身(返回值可继续索引 / 调用,
    // 如 `local t = mineral.timer.after(10, fn); t:stop()`)。
    let inner = absorber.clone();
    meta.set(
        "__call",
        lua.create_function(move |_, _: MultiValue| Ok(inner.clone()))?,
    )?;
    absorber.set_metatable(Some(meta));
    lua.globals().set("mineral", absorber)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::inject_noop_host;

    /// 守卫:用户 config.lua 顶层对 `mineral` 的**任意**用法(已知 API、
    /// 未来新增、甚至拼错的名字)在非 daemon 进程 eval 都不报错——否则
    /// 整份配置回落默认,keys / theme 全丢。
    #[test]
    fn absorber_stub_swallows_any_usage() -> color_eyre::Result<()> {
        let lua = mlua::Lua::new();
        inject_noop_host(&lua)?;
        lua.load(
            r#"
            -- 现有 API 表面
            mineral.on("track_finished", function(args) end)
            mineral.action("x.y", function(ctx) end)
            mineral.observe("player.volume", function(v) end)
            mineral.get("player.volume")
            mineral.player.toggle(); mineral.player.play("netease:1")
            mineral.ui.toast("hi", { id = "t" })
            mineral.log.info("i")
            mineral.store.inc("netease:1", "plugin.x", 1, function(v, err) end)
            mineral.queue.list(function(q, err) end)
            mineral.library.love("netease:1", true)
            -- 返回值可继续链式使用(timer handle 范式)
            local t = mineral.timer.every(1000, function() end)
            t:stop(); t:resume(); t:kill()
            -- 未来新增 / 拼错的名字同样静默(零同步压力的设计本意)
            mineral.future_api.deeply.nested("arg"):chained():more()
            "#,
        )
        .exec()?;
        Ok(())
    }
}

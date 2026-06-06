//! 脚本回调的看门狗:指令计数 hook + 墙钟双阈值。
//!
//! 每次调 Lua 回调前装 hook(每 N 条 VM 指令检查一次墙钟),超
//! `soft_wall` 记一次 warn 日志继续跑,超 `hard_wall` 让本次调用以
//! Lua 错误中断;调用结束(无论成败)摘 hook。**VM 本身保留**——
//! 中断的只是这一次回调,后续事件照常分发。

use std::cell::Cell;
use std::time::Instant;

use mlua::{FromLuaMulti, HookTriggers, IntoLuaMulti, Lua, VmState};

/// 看门狗参数。全必填(默认值是 PR-4 配置三件套的事,
/// `default.lua` 是唯一默认真相源,这里不重复)。
#[derive(Clone, Copy, Debug, derive_getters::Getters, typed_builder::TypedBuilder)]
pub struct WatchdogConfig {
    /// 每多少条 VM 指令检查一次墙钟(越小越灵敏、开销越大)。
    instruction_interval: u32,

    /// 软阈值:超过记一次 warn 日志,继续执行。
    soft_wall: std::time::Duration,

    /// 硬阈值:超过中断本次回调(Lua 错误冒泡到 dispatch 层)。
    hard_wall: std::time::Duration,
}

/// 在看门狗保护下调用一个 Lua 函数。
///
/// # Params:
///   - `lua`: 目标 VM(hook 装在整个 VM 上,调用结束后摘除)
///   - `cfg`: 双阈值参数
///   - `func`: 要调用的 Lua 函数
///   - `args`: 实参
///
/// # Return:
///   函数返回值;回调出错或超 `hard_wall` 被中断时为 `Err`。
pub(crate) fn call_guarded<A, R>(
    lua: &Lua,
    cfg: &WatchdogConfig,
    func: &mlua::Function,
    args: A,
) -> mlua::Result<R>
where
    A: IntoLuaMulti,
    R: FromLuaMulti,
{
    let start = Instant::now();
    let soft_wall = *cfg.soft_wall();
    let hard_wall = *cfg.hard_wall();
    // hook 是 Fn(不可变),软阈值「只警一次」的状态用 Cell 携带。
    let soft_warned = Cell::new(false);
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(*cfg.instruction_interval()),
        move |_lua, _debug| {
            let elapsed = start.elapsed();
            if elapsed >= hard_wall {
                return Err(mlua::Error::RuntimeError(format!(
                    "script callback exceeded hard wall ({hard_wall:?}), interrupted"
                )));
            }
            if elapsed >= soft_wall && !soft_warned.get() {
                soft_warned.set(true);
                mineral_log::warn!(
                    target: "script",
                    elapsed_ms = elapsed.as_millis(),
                    "script callback exceeded soft wall, still running"
                );
            }
            Ok(VmState::Continue)
        },
    );
    let result = func.call::<R>(args);
    lua.remove_hook();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用的灵敏看门狗:1000 条指令一查,软 10ms / 硬 50ms。
    fn tight_config() -> WatchdogConfig {
        WatchdogConfig::builder()
            .instruction_interval(1_000)
            .soft_wall(std::time::Duration::from_millis(10))
            .hard_wall(std::time::Duration::from_millis(50))
            .build()
    }

    #[test]
    fn busy_loop_interrupted_by_hard_wall() -> color_eyre::Result<()> {
        let lua = Lua::new();
        let func = lua
            .load("return function() while true do end end")
            .eval::<mlua::Function>()?;
        let started = Instant::now();
        let result = call_guarded::<_, ()>(&lua, &tight_config(), &func, ());
        assert!(result.is_err(), "死循环必须被硬阈值中断");
        // 远超硬阈值才返回说明 hook 没生效;给 10 倍裕量吸收调度抖动。
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "中断耗时 {:?},硬阈值未生效",
            started.elapsed()
        );
        Ok(())
    }

    #[test]
    fn vm_stays_usable_after_interrupt() -> color_eyre::Result<()> {
        let lua = Lua::new();
        let spin = lua
            .load("return function() while true do end end")
            .eval::<mlua::Function>()?;
        let result = call_guarded::<_, ()>(&lua, &tight_config(), &spin, ());
        assert!(result.is_err(), "死循环必须被中断");
        // 中断后 VM 保留:正常回调照常工作,hook 已摘不再误伤。
        let add = lua
            .load("return function(a, b) return a + b end")
            .eval::<mlua::Function>()?;
        let sum = call_guarded::<_, i64>(&lua, &tight_config(), &add, (2, 3))?;
        assert_eq!(sum, 5);
        Ok(())
    }
}

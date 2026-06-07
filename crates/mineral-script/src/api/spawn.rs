//! `mineral.spawn(args, opts?, fn)`:结构化异步子进程(回调风格)。
//!
//! `args` 是字符串数组(首元素为可执行文件),**不拼 shell 串**;
//! 返回句柄 table,`handle:kill()` 中止在跑的子进程。

use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;
use crate::proc::SpawnSpec;

/// 把 `spawn` 挂到 `mineral` 表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `mineral`: 全局 `mineral` 表
///   - `host`: 宿主句柄(命令出口 + 在途查询表)
pub(crate) fn install(lua: &Lua, mineral: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    mineral.set(
        "spawn",
        lua.create_function(
            move |lua, (args, second, third): (Table, mlua::Value, Option<mlua::Function>)| {
                let (opts, callback) = split_args(second, third)?;
                let spec = parse_spec(&args, opts.as_ref())?;
                let id = h.events.lock().next_spawn_id();
                let query = h.register_query(lua, callback)?;
                let _ = h.commands.send(ScriptCmd::Spawn { id, spec, query });
                make_handle(lua, &h, id)
            },
        )?,
    )
}

/// 拆中置可选参:`spawn(args, fn)` 与 `spawn(args, opts, fn)` 两种调用形。
fn split_args(
    second: mlua::Value,
    third: Option<mlua::Function>,
) -> mlua::Result<(Option<Table>, mlua::Function)> {
    match (second, third) {
        (mlua::Value::Function(callback), None) => Ok((None, callback)),
        (mlua::Value::Table(opts), Some(callback)) => Ok((Some(opts), callback)),
        (mlua::Value::Nil, Some(callback)) => Ok((None, callback)),
        _ => Err(mlua::Error::runtime(
            "用法:spawn(args, fn) 或 spawn(args, opts, fn)",
        )),
    }
}

/// 把 `args` 数组 + opts table 解析成结构化 [`SpawnSpec`]。
fn parse_spec(args: &Table, opts: Option<&Table>) -> mlua::Result<SpawnSpec> {
    let argv = args
        .clone()
        .sequence_values::<String>()
        .collect::<mlua::Result<Vec<String>>>()?;
    let mut iter = argv.into_iter();
    let Some(program) = iter.next() else {
        return Err(mlua::Error::runtime("spawn 的 args 不能为空"));
    };
    let mut spec = SpawnSpec {
        program,
        args: iter.collect(),
        cwd: None,
        env: Vec::new(),
    };
    if let Some(table) = opts {
        spec.cwd = table
            .get::<Option<String>>("cwd")?
            .map(std::path::PathBuf::from);
        if let Some(env) = table.get::<Option<Table>>("env")? {
            for pair in env.pairs::<String, String>() {
                let (key, value) = pair?;
                spec.env.push((key, value));
            }
        }
    }
    Ok(spec)
}

/// 造子进程句柄 table:`handle:kill()` 发中止命令(fire-and-forget)。
fn make_handle(lua: &Lua, host: &ScriptHost, id: crate::proc::SpawnId) -> mlua::Result<Table> {
    let handle = lua.create_table()?;
    let h = host.clone();
    handle.set(
        "kill",
        // 冒号调用收 self,丢弃即可。
        lua.create_function(move |_lua, _this: mlua::Value| {
            let _ = h.commands.send(ScriptCmd::SpawnKill { id });
            Ok(())
        })?,
    )?;
    Ok(handle)
}

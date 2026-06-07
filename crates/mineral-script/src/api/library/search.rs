//! `mineral.library.search(query, opts?, fn)`:按关键词搜索歌曲(回调风格)。

use mineral_model::SourceKind;
use mlua::{Lua, Table};

use crate::host::ScriptHost;
use crate::message::ScriptCmd;

/// opts 缺省时的单页上限(与 channel 层 `Page::default` 的 limit 一致)。
const DEFAULT_LIMIT: u32 = 30;

/// 把 `search` 挂到 `library` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `library`: `mineral.library` 子表
///   - `host`: 宿主句柄(命令出口 + 在途查询表)
pub(crate) fn install(lua: &Lua, library: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let h = host.clone();
    library.set(
        "search",
        lua.create_function(
            move |lua, (term, second, third): (String, mlua::Value, Option<mlua::Function>)| {
                let (opts, callback) = split_args(second, third)?;
                let (source, offset, limit) = parse_opts(opts.as_ref())?;
                let query = h.register_query(lua, callback)?;
                let _ = h.commands.send(ScriptCmd::LibrarySearch {
                    term,
                    source,
                    offset,
                    limit,
                    query,
                });
                Ok(())
            },
        )?,
    )
}

/// 拆中置可选参:`search(q, fn)` 与 `search(q, opts, fn)` 两种调用形。
///
/// # Params:
///   - `second`: 第二实参(opts table 或回调)
///   - `third`: 第三实参(opts 形态下的回调)
///
/// # Return:
///   `(opts, 回调)`;形态不合法报脚本错误。
fn split_args(
    second: mlua::Value,
    third: Option<mlua::Function>,
) -> mlua::Result<(Option<Table>, mlua::Function)> {
    match (second, third) {
        (mlua::Value::Function(callback), None) => Ok((None, callback)),
        (mlua::Value::Table(opts), Some(callback)) => Ok((Some(opts), callback)),
        (mlua::Value::Nil, Some(callback)) => Ok((None, callback)),
        _ => Err(mlua::Error::runtime(
            "用法:search(query, fn) 或 search(query, opts, fn)",
        )),
    }
}

/// 解析 opts table(全字段可缺省)。
///
/// `source` 经 `SourceKind::from_name` 解析——未知名 intern 成新 SourceKind,
/// daemon 侧查不到对应 channel 时回调收 `(nil, err)`,不在这里挡。
///
/// # Params:
///   - `opts`: opts table(`None` = 全默认)
///
/// # Return:
///   `(source, offset, limit)`。
fn parse_opts(opts: Option<&Table>) -> mlua::Result<(Option<SourceKind>, u32, u32)> {
    let Some(table) = opts else {
        return Ok((None, /*offset*/ 0, DEFAULT_LIMIT));
    };
    let source = table
        .get::<Option<String>>("source")?
        .map(|name| SourceKind::from_name(&name));
    let offset = table.get::<Option<u32>>("offset")?.unwrap_or(0);
    let limit = table.get::<Option<u32>>("limit")?.unwrap_or(DEFAULT_LIMIT);
    Ok((source, offset, limit))
}

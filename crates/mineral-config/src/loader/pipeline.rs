//! Lua 配置的求值、深合并和落型;默认内容与回调摘取规则由宿主传入。

use std::path::Path;

use mlua::{Lua, Table, Value};
use serde::de::DeserializeOwned;

use super::merge::deep_merge;
use super::tree::deserialize_tree;
use super::warning::ConfigWarning;
use crate::{Error, Result};

/// 文件加载结果;宿主根据 user_loaded 决定是否激活本次 VM。
pub struct FileLoad<T> {
    /// 已落型的配置;用户文件失败或缺失时使用宿主默认。
    pub config: T,

    /// 用户配置读取、求值或落型失败的诊断。
    pub warnings: Vec<ConfigWarning>,

    /// 是否存在且成功加载了用户文件。
    pub user_loaded: bool,

    /// 与强类型配置一致的数据树;回调已由宿主摘取。
    pub tree: serde_json::Value,
}

/// 在指定 VM 加载一个文件;不执行 setup,也不读取其他宿主的文件。
/// 用户文件缺失或失败时返回默认和诊断;默认配置损坏时返回错误。
/// prepare 在落型前摘取宿主回调,并标记需要明确数组语义的 Lua 表。
pub fn load_file<T: DeserializeOwned>(
    lua: &Lua,
    user_path: &Path,
    default_source: &str,
    default_name: &str,
    prepare: fn(&Lua, &Table) -> mlua::Result<()>,
) -> Result<FileLoad<T>> {
    let default = eval_default(lua, default_source, default_name)?;
    let user = eval_user(lua, user_path);
    let mut warnings = Vec::<ConfigWarning>::new();
    match user {
        Ok(Some(user)) => {
            let merged = deep_merge(lua, default.clone(), user)
                .and_then(|merged| {
                    prepare(lua, &merged)?;
                    Ok(merged)
                })
                .map_err(|source| ConfigWarning::Eval {
                    path: user_path.to_path_buf(),
                    source,
                })
                .and_then(from_lua_table);
            match merged {
                Ok((config, tree)) => {
                    return Ok(FileLoad {
                        config,
                        warnings,
                        user_loaded: true,
                        tree,
                    });
                }
                Err(warning) => warnings.push(warning),
            }
        }
        Ok(None) => {}
        Err(warning) => warnings.push(warning),
    }
    let (config, tree) = finalize_default(lua, default, prepare)?;
    Ok(FileLoad {
        config,
        warnings,
        user_loaded: false,
        tree,
    })
}

/// 求值指定宿主源码,合并默认并落型;失败不返回部分成功产物。
/// 调用方负责执行预算、API 安装与失败时丢弃 VM 和暂存效果。
pub fn from_source<T: DeserializeOwned>(
    lua: &Lua,
    source: &str,
    name: &str,
    default_source: &str,
    default_name: &str,
    prepare: fn(&Lua, &Table) -> mlua::Result<()>,
) -> Result<(T, serde_json::Value)> {
    let default = eval_default(lua, default_source, default_name)?;
    let user = lua.load(source).set_name(name).eval::<Table>()?;
    let merged = deep_merge(lua, default, user)?;
    prepare(lua, &merged)?;
    from_lua_table(merged).map_err(|warning| Error::InvalidConfig { warning })
}

/// 只求值宿主内置默认;函数摘取后同时返回强类型配置与数据树。
pub fn defaults<T: DeserializeOwned>(
    source: &str,
    name: &str,
    prepare: fn(&Lua, &Table) -> mlua::Result<()>,
) -> Result<(T, serde_json::Value)> {
    let lua = Lua::new();
    let default = eval_default(&lua, source, name)?;
    finalize_default(&lua, default, prepare)
}

/// 默认表无法落型属于程序包错误,不作为用户文件告警。
fn finalize_default<T: DeserializeOwned>(
    lua: &Lua,
    default: Table,
    prepare: fn(&Lua, &Table) -> mlua::Result<()>,
) -> Result<(T, serde_json::Value)> {
    prepare(lua, &default)?;
    from_lua_table(default).map_err(|warning| Error::DefaultConfig { warning })
}

/// 求值宿主内置默认;name 为错误定位提供默认文件名。
fn eval_default(lua: &Lua, source: &str, name: &str) -> Result<Table> {
    lua.load(source)
        .set_name(name)
        .eval()
        .map_err(|source| Error::Lua {
            operation: "求值内置默认配置",
            source,
        })
}

/// 只读取指定用户文件;缺失正常,其他读取或求值失败携带文件路径。
fn eval_user(lua: &Lua, path: &Path) -> std::result::Result<Option<Table>, ConfigWarning> {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigWarning::Read {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    lua.load(&source)
        .set_name(path.to_string_lossy())
        .eval::<Table>()
        .map(Some)
        .map_err(|source| ConfigWarning::Eval {
            path: path.to_path_buf(),
            source,
        })
}

/// 回调摘取后的 Lua 表转换成数据树,再按宿主 schema 精确落型。
fn from_lua_table<T: DeserializeOwned>(
    table: Table,
) -> std::result::Result<(T, serde_json::Value), ConfigWarning> {
    let tree = serde_json::to_value(Value::Table(table))
        .map_err(|source| ConfigWarning::Serialize { source })?;
    let config = deserialize_tree(&tree)?;
    Ok((config, tree))
}

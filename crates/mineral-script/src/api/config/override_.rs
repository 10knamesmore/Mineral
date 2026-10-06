//! 两个宿主共用的配置覆盖解析。
//!
//! `override(patch)` 把配置偏表拍平成叶子，数组整体替换；
//! `override(path, value)` 支持动态路径和 nil 撤销。
//! 两个宿主各自校验并更新本进程配置，均不改配置文件，也不传送配置树。

use mineral_protocol::BusValue;
use mlua::{Lua, Table};

use crate::api::value::lua_to_bus;
use crate::message::ConfigOverrideOp;

/// 把 `override` 挂到 `config` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `config`: `mineral.config` 子表
///   - `send`: 宿主接收已解析的覆盖操作
pub(crate) fn install(
    lua: &Lua,
    config: &Table,
    send: impl Fn(Vec<ConfigOverrideOp>) + Send + 'static,
) -> mlua::Result<()> {
    config.set(
        "override",
        lua.create_function(move |_lua, (target, value): (mlua::Value, mlua::Value)| {
            let ops = match &target {
                mlua::Value::String(path) => {
                    vec![string_form_op(path.to_str()?.to_owned(), &value)?]
                }
                mlua::Value::Table(_) => patch_to_ops(&target)?,
                other => {
                    return Err(mlua::Error::RuntimeError(format!(
                        "override 首参须是配置路径字符串或配置偏表,实得 {}",
                        other.type_name()
                    )));
                }
            };
            // 空补丁表拍不出叶子，不产生宿主命令。
            if ops.is_empty() {
                return Ok(());
            }
            send(ops);
            Ok(())
        })?,
    )
}

/// 字符串形:`(path, value)` 收敛成一条叶子 op。
///
/// # Params:
///   - `path`: 配置路径
///   - `value`: 覆盖值;nil 收敛成「撤销」——不产生 `Some(Nil)`,避免
///     「覆盖成 Nil」与「撤销」两义
fn string_form_op(path: String, value: &mlua::Value) -> mlua::Result<ConfigOverrideOp> {
    let value = match value {
        mlua::Value::Nil => None,
        other => Some(lua_to_bus(other, /*depth*/ 0)?),
    };
    Ok(ConfigOverrideOp { path, value })
}

/// 表对象形:配置偏表拍平成标量粒度的叶子 op 列表。
///
/// 拍出的叶子与在标量粒度手写字符串形完全等价(daemon 走同一条合并 /
/// 校验路),粒度越细,落型失败时按 path 剔除越精确。
///
/// # Params:
///   - `patch`: 配置偏表(Lua table)
///
/// # Return:
///   叶子 op 列表;顶层不是字符串键的表报 Lua 错。
fn patch_to_ops(patch: &mlua::Value) -> mlua::Result<Vec<ConfigOverrideOp>> {
    let BusValue::Map(entries) = lua_to_bus(patch, /*depth*/ 0)? else {
        return Err(mlua::Error::RuntimeError(
            "配置偏表须是字符串键的表(数组不是合法配置补丁)".to_owned(),
        ));
    };
    let mut ops = Vec::new();
    flatten_into("", entries, &mut ops);
    Ok(ops)
}

/// 递归拍平:string-key 子表带 `prefix.key` 下钻,其余(数组 / 标量)产出
/// 一条叶子。数组整体替换不下钻,与配置「数组整体替换」语义一致。
fn flatten_into(prefix: &str, entries: Vec<(String, BusValue)>, ops: &mut Vec<ConfigOverrideOp>) {
    for (key, value) in entries {
        let path = if prefix.is_empty() {
            key
        } else {
            format!("{prefix}.{key}")
        };
        match value {
            BusValue::Map(children) => flatten_into(&path, children, ops),
            // Lua 表存不下 nil value,拍平叶子里 Nil 不可达;防御性跳过,
            // 保住「表形永不产生撤销 / 不发 Some(Nil)」两条不变量。
            BusValue::Nil => {}
            leaf => ops.push(ConfigOverrideOp {
                path,
                value: Some(leaf),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::BusValue;

    use crate::api::test_support::{drain_cmds, vm_with_commands};
    use crate::message::{ConfigOverrideOp, ScriptCmd};

    /// 造一条覆盖叶子 op(测试简写)。
    fn op(path: &str, value: BusValue) -> ConfigOverrideOp {
        ConfigOverrideOp {
            path: path.to_owned(),
            value: Some(value),
        }
    }

    /// 断言命令流里恰有一条 `ConfigOverride`,取出其 ops 并按 path 排序
    /// (Lua 表遍历顺序不定;同层叶子 path 互异,merge 语义下可交换)。
    fn sole_override_ops(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<ScriptCmd>,
    ) -> color_eyre::Result<Vec<ConfigOverrideOp>> {
        let mut cmds = drain_cmds(rx);
        let sole = cmds.pop();
        match (sole, cmds.is_empty()) {
            (Some(ScriptCmd::ConfigOverride { mut ops }), true) => {
                ops.sort_by(|a, b| a.path.cmp(&b.path));
                Ok(ops)
            }
            (sole, _) => {
                color_eyre::eyre::bail!("应恰有一条 ConfigOverride,实得 {cmds:?} + {sole:?}");
            }
        }
    }

    #[test]
    fn string_form_sends_single_op() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"local mineral = require("mineral.daemon"); mineral.config.override("audio.volume", 2)"#)
            .exec()?;
        assert_eq!(
            drain_cmds(&mut cmd_rx),
            vec![ScriptCmd::ConfigOverride {
                ops: vec![op("audio.volume", BusValue::Int(2))],
            }]
        );
        Ok(())
    }

    #[test]
    fn string_form_nil_converges_to_revoke() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"local mineral = require("mineral.daemon"); mineral.config.override("audio.volume", nil)"#)
            .exec()?;
        assert_eq!(
            drain_cmds(&mut cmd_rx),
            vec![ScriptCmd::ConfigOverride {
                ops: vec![ConfigOverrideOp {
                    path: "audio.volume".to_owned(),
                    value: None,
                }],
            }],
            "nil 必须收敛成撤销(None),不得发 Some(Nil)"
        );
        Ok(())
    }

    #[test]
    fn string_form_rejects_function_value() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua
            .load(r#"local mineral = require("mineral.daemon"); mineral.config.override("k", function() end)"#)
            .exec();
        assert!(result.is_err(), "function 值必须报 Lua 错");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }

    #[test]
    fn table_form_flattens_nested_patch_to_scalar_leaves() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(
            r#"local mineral = require("mineral.daemon")
            mineral.config.override({
                audio = { volume = 2, engine_tick_ms = 1 },
                download = { tagging = false },
            })"#,
        )
        .exec()?;
        assert_eq!(
            sole_override_ops(&mut cmd_rx)?,
            vec![
                op("audio.engine_tick_ms", BusValue::Int(1)),
                op("audio.volume", BusValue::Int(2)),
                op("download.tagging", BusValue::Bool(false)),
            ],
            "多叶子拍到标量粒度,一次调用一条命令"
        );
        Ok(())
    }

    #[test]
    fn table_form_array_is_one_leaf() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"local mineral = require("mineral.daemon"); mineral.config.override({ sources = { ["local"] = { roots = { "a", "b" } } } })"#)
            .exec()?;
        assert_eq!(
            sole_override_ops(&mut cmd_rx)?,
            vec![op(
                "sources.local.roots",
                BusValue::Array(vec![
                    BusValue::Str("a".to_owned()),
                    BusValue::Str("b".to_owned()),
                ]),
            )],
            "数组是叶子,整体替换不下钻"
        );
        Ok(())
    }

    #[test]
    fn table_form_false_is_override_not_revoke() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"local mineral = require("mineral.daemon"); mineral.config.override({ download = { tagging = false } })"#)
            .exec()?;
        assert_eq!(
            sole_override_ops(&mut cmd_rx)?,
            vec![op("download.tagging", BusValue::Bool(false))],
            "falsy 值是覆盖不是撤销;表形永不产生 value = None"
        );
        Ok(())
    }

    #[test]
    fn table_form_empty_patch_sends_nothing() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"local mineral = require("mineral.daemon"); mineral.config.override({})"#)
            .exec()?;
        lua.load(
            r#"local mineral = require("mineral.daemon"); mineral.config.override({ audio = {} })"#,
        )
        .exec()?;
        assert!(
            drain_cmds(&mut cmd_rx).is_empty(),
            "无叶子的补丁不产生宿主命令"
        );
        Ok(())
    }

    #[test]
    fn table_form_rejects_array_patch() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua
            .load(r#"local mineral = require("mineral.daemon"); mineral.config.override({ 1, 2 })"#)
            .exec();
        assert!(result.is_err(), "顶层数组不是合法配置补丁");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }

    #[test]
    fn table_form_rejects_function_leaf() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua
            .load(r#"local mineral = require("mineral.daemon"); mineral.config.override({ x = function() end })"#)
            .exec();
        assert!(result.is_err(), "function 叶子必须报 Lua 错");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }

    #[test]
    fn table_form_ignores_extra_args() -> color_eyre::Result<()> {
        // Lua 惯例:多余实参静默丢弃(签名只收两参,第三参本就到不了 Rust);
        // 表形的第二参不搞特判,与之保持一致。
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"local mineral = require("mineral.daemon"); mineral.config.override({ download = { tagging = true } }, 3)"#)
            .exec()?;
        assert_eq!(
            sole_override_ops(&mut cmd_rx)?,
            vec![op("download.tagging", BusValue::Bool(true))],
            "补丁照常拍平下发,多余参数不影响"
        );
        Ok(())
    }

    #[test]
    fn override_rejects_number_target() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua
            .load(r#"local mineral = require("mineral.daemon"); mineral.config.override(42, 1)"#)
            .exec();
        assert!(result.is_err(), "首参只认路径字符串或补丁表");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }

    /// 表对象形与逐条字符串形产生相同叶子，未覆盖的配置字段保留。
    #[test]
    fn table_form_effective_tree_matches_string_form() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"local mineral = require("mineral.daemon"); mineral.config.override({ b = { c = 5, d = true } })"#)
            .exec()?;
        let table_ops = sole_override_ops(&mut cmd_rx)?;

        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(
            r#"
            local mineral = require("mineral.daemon")
            mineral.config.override("b.c", 5)
            mineral.config.override("b.d", true)
            "#,
        )
        .exec()?;
        let mut string_ops = drain_cmds(&mut cmd_rx)
            .into_iter()
            .flat_map(|cmd| {
                if let ScriptCmd::ConfigOverride { ops } = cmd {
                    ops
                } else {
                    Vec::new()
                }
            })
            .collect::<Vec<ConfigOverrideOp>>();
        string_ops.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(
            table_ops, string_ops,
            "表形拍到标量粒度,与手写字符串形逐条相同"
        );

        let mut tree = serde_json::json!({ "a": 1, "b": { "c": 2 }  });
        for leaf in &table_ops {
            let Some(value) = leaf.value.clone() else {
                color_eyre::eyre::bail!("表形不得产生撤销叶子");
            };
            tree = mineral_config::merge_tree(
                tree,
                mineral_config::nest_path(&leaf.path, value.into_json()),
            );
        }
        assert_eq!(
            tree,
            serde_json::json!({ "a": 1, "b": { "c": 5, "d": true }  }),
            "merge 后有效树:未 touch 字段保留,新叶子并入"
        );
        Ok(())
    }
}

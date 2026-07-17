//! `mineral.config.override`:session 级配置覆盖,两种调用形态。
//!
//! 表对象形 `override(patch)` 收一张配置偏表(结构同 config.lua 返回表,
//! LuaLS 能给全程补全),拍平成标量粒度的叶子 `(path, value)` 批量上 wire;
//! 字符串形 `override(path, value)` 保留(动态 path / `value = nil` 撤销,
//! 回落配置文件的值)。两形拍出的叶子走 daemon 同一条路:深合并进有效配置
//! 并落型校验(坏路径 / 坏值被剔除 + 警告),结果经 ConfigChanged 推给订阅
//! client;不碰配置文件本身,daemon 重启即清。

use mineral_protocol::BusValue;
use mlua::{Lua, Table};

use crate::api::value::lua_to_bus;
use crate::host::ScriptHost;
use crate::message::{ConfigOverrideOp, ScriptCmd};

/// 把 `override` 挂到 `config` 子表上。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `config`: `mineral.config` 子表
///   - `host`: 宿主句柄(闭包捕获其命令出口)
pub(crate) fn install(lua: &Lua, config: &Table, host: &ScriptHost) -> mlua::Result<()> {
    let commands = host.commands.clone();
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
            // 空补丁表拍不出叶子,不上 wire(daemon 侧本也是 no-op)。
            if ops.is_empty() {
                return Ok(());
            }
            // 接收端关闭(daemon 停机)时静默丢,脚本不感知。
            let _ = commands.send(ScriptCmd::ConfigOverride { ops });
            Ok(())
        })?,
    )
}

/// 字符串形:`(path, value)` 收敛成一条叶子 op。
///
/// # Params:
///   - `path`: 配置路径
///   - `value`: 覆盖值;nil 收敛成「撤销」——`Some(Nil)` 不上 wire,避免
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
                color_eyre::eyre::bail!("应恰有一条 ConfigOverride,实得 {cmds:?} + {sole:?}")
            }
        }
    }

    #[test]
    fn string_form_sends_single_op() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.config.override("tui.lyrics.fullscreen_line_gap", 2)"#)
            .exec()?;
        assert_eq!(
            drain_cmds(&mut cmd_rx),
            vec![ScriptCmd::ConfigOverride {
                ops: vec![op("tui.lyrics.fullscreen_line_gap", BusValue::Int(2))],
            }]
        );
        Ok(())
    }

    #[test]
    fn string_form_nil_converges_to_revoke() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.config.override("tui.lyrics.fullscreen_line_gap", nil)"#)
            .exec()?;
        assert_eq!(
            drain_cmds(&mut cmd_rx),
            vec![ScriptCmd::ConfigOverride {
                ops: vec![ConfigOverrideOp {
                    path: "tui.lyrics.fullscreen_line_gap".to_owned(),
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
            .load(r#"mineral.config.override("k", function() end)"#)
            .exec();
        assert!(result.is_err(), "function 值必须报 Lua 错");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }

    #[test]
    fn table_form_single_leaf() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.config.override({ tui = { waveform = { enabled = true } } })"#)
            .exec()?;
        assert_eq!(
            sole_override_ops(&mut cmd_rx)?,
            vec![op("tui.waveform.enabled", BusValue::Bool(true))]
        );
        Ok(())
    }

    #[test]
    fn table_form_flattens_nested_patch_to_scalar_leaves() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(
            r#"mineral.config.override({
                tui = {
                    lyrics = { fullscreen_line_gap = 2, compact_line_gap = 1 },
                    waveform = { enabled = false },
                },
            })"#,
        )
        .exec()?;
        assert_eq!(
            sole_override_ops(&mut cmd_rx)?,
            vec![
                op("tui.lyrics.compact_line_gap", BusValue::Int(1)),
                op("tui.lyrics.fullscreen_line_gap", BusValue::Int(2)),
                op("tui.waveform.enabled", BusValue::Bool(false)),
            ],
            "多叶子拍到标量粒度,一次调用一条命令"
        );
        Ok(())
    }

    #[test]
    fn table_form_array_is_one_leaf() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.config.override({ tui = { flags = { "a", "b" } } })"#)
            .exec()?;
        assert_eq!(
            sole_override_ops(&mut cmd_rx)?,
            vec![op(
                "tui.flags",
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
        lua.load(r#"mineral.config.override({ tui = { waveform = { enabled = false } } })"#)
            .exec()?;
        assert_eq!(
            sole_override_ops(&mut cmd_rx)?,
            vec![op("tui.waveform.enabled", BusValue::Bool(false))],
            "falsy 值是覆盖不是撤销;表形永不产生 value = None"
        );
        Ok(())
    }

    #[test]
    fn table_form_empty_patch_sends_nothing() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.config.override({})"#).exec()?;
        lua.load(r#"mineral.config.override({ tui = {} })"#)
            .exec()?;
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "无叶子的补丁不上 wire");
        Ok(())
    }

    #[test]
    fn table_form_rejects_array_patch() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua.load(r#"mineral.config.override({ 1, 2 })"#).exec();
        assert!(result.is_err(), "顶层数组不是合法配置补丁");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }

    #[test]
    fn table_form_rejects_function_leaf() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua
            .load(r#"mineral.config.override({ tui = { x = function() end } })"#)
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
        lua.load(r#"mineral.config.override({ tui = { waveform = { enabled = true } } }, 3)"#)
            .exec()?;
        assert_eq!(
            sole_override_ops(&mut cmd_rx)?,
            vec![op("tui.waveform.enabled", BusValue::Bool(true))],
            "补丁照常拍平下发,多余参数不影响"
        );
        Ok(())
    }

    #[test]
    fn override_rejects_number_target() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        let result = lua.load(r#"mineral.config.override(42, 1)"#).exec();
        assert!(result.is_err(), "首参只认路径字符串或补丁表");
        assert!(drain_cmds(&mut cmd_rx).is_empty(), "报错时不得发命令");
        Ok(())
    }

    /// 「同一条 daemon 路」不变量:表对象形拍平的 ops 与在标量粒度手写字符串形
    /// 逐条相同;逐条 nest + merge 出的有效树也一致(未 touch 字段保留)。
    #[test]
    fn table_form_effective_tree_matches_string_form() -> color_eyre::Result<()> {
        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(r#"mineral.config.override({ tui = { b = { c = 5, d = true } } })"#)
            .exec()?;
        let table_ops = sole_override_ops(&mut cmd_rx)?;

        let (lua, mut cmd_rx) = vm_with_commands()?;
        lua.load(
            r#"
            mineral.config.override("tui.b.c", 5)
            mineral.config.override("tui.b.d", true)
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

        let mut tree = serde_json::json!({ "tui": { "a": 1, "b": { "c": 2 } } });
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
            serde_json::json!({ "tui": { "a": 1, "b": { "c": 5, "d": true } } }),
            "merge 后有效树:未 touch 字段保留,新叶子并入"
        );
        Ok(())
    }

    /// meta stub 守卫:`override` 必须以表对象形为主签名(`patch: mineral.Config`,
    /// LuaLS 才能给补丁表全程补全)并保留字符串形 overload。
    #[test]
    fn meta_stub_override_annotates_both_forms() -> color_eyre::Result<()> {
        use color_eyre::eyre::WrapErr;
        let meta_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../mineral-config/src/lua/meta/mineral.lua"
        );
        let meta = std::fs::read_to_string(meta_path).wrap_err("read meta/mineral.lua")?;
        assert!(
            meta.contains("---@param patch mineral.Config"),
            "表对象形主签名标注缺失(patch: mineral.Config)"
        );
        assert!(
            meta.contains("---@overload fun(path: string, value: mineral.BusPayload|nil)"),
            "字符串形 overload 标注缺失"
        );
        Ok(())
    }
}

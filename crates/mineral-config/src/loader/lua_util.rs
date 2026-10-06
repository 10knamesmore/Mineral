//! Lua 表在 Rust 侧的导航辅助:配置表手术(函数摘取等)共用的小件。

use mlua::{Function, Lua, Table, Value};

/// 摘取可选根 setup 到宿主指定的 registry 键;不执行函数。
/// 非函数值留在配置表中,由宿主 schema 报未知字段。
pub fn extract_setup(lua: &Lua, root: &Table, registry: &str) -> mlua::Result<()> {
    let mut setup = Value::Nil;
    if let Ok(function) = root.get::<Function>("setup") {
        setup = Value::Function(function);
        root.raw_set("setup", Value::Nil)?;
    }
    lua.set_named_registry_value(registry, setup)
}

/// 顺路径逐级取子表;两种中断都收敛为 `None`,但区别对待:
/// **键缺失**(用户没配这段,常态)静默;**键存在但不是表**(形态错)打带
/// 断点路径的 debug 日志——正式报错留给落型阶段的带路径 warning,不在此重复。
/// (`Table` 是 VM registry 句柄,clone 只是句柄复制,不拷贝表内容。)
///
/// # Params:
///   - `root`: 起点表
///   - `path`: 逐级键名
///
/// # Return:
///   路径尽头的子表;缺失 / 形态错为 `None`。
pub fn table_path(root: &Table, path: &[&str]) -> Option<Table> {
    let mut current = root.clone();
    for (depth, key) in path.iter().enumerate() {
        // get::<Value> 对缺失键给 Nil,不报错;真正的 Err 只剩 VM 级故障。
        match current.get::<Value>(*key) {
            Ok(Value::Table(next)) => current = next,
            Ok(Value::Nil) => return None,
            Ok(other) => {
                mineral_log::debug!(
                    target: "config",
                    path = path.get(..=depth).unwrap_or_default().join("."),
                    got = other.type_name(),
                    "路径中断:节点不是表,跳过提取"
                );
                return None;
            }
            Err(e) => {
                mineral_log::debug!(
                    target: "config",
                    path = path.get(..=depth).unwrap_or_default().join("."),
                    error = mineral_log::chain(&e),
                    "路径读取失败,跳过提取"
                );
                return None;
            }
        }
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use mlua::Lua;

    use super::table_path;

    /// 全路径命中;缺失键与非表节点都返回 None,形态错误只记 debug 日志。
    #[test]
    fn walks_path_and_misses_quietly() -> color_eyre::Result<()> {
        let lua = Lua::new();
        let root: mlua::Table = lua
            .load(r#"{ copy = { templates = { 1 } }, flat = 5 }"#)
            .eval()?;
        let hit = table_path(&root, &["copy", "templates"]);
        assert_eq!(hit.map(|t| t.raw_len()), Some(1), "全路径命中");
        assert!(
            table_path(&root, &["copy", "missing"]).is_none(),
            "缺失键静默 None"
        );
        assert!(
            table_path(&root, &["flat", "deeper"]).is_none(),
            "非表节点 None(仅 debug 日志)"
        );
        Ok(())
    }
}

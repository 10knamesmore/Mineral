//! Lua 表在 Rust 侧的导航辅助:配置表手术(函数摘取等)共用的小件。

use mlua::{Table, Value};

/// [`table_path`] 的点号语法糖:`table_at!(merged, tui.copy.templates)`
/// = `table_path(merged, &["tui", "copy", "templates"])`。键须是合法标识符;
/// 将来出现带 `-` 等的键再给字符串字面量分支。
/// (与函数不同名是刻意的:`use` 再导出宏时,同名函数会在值命名空间撞车。)
macro_rules! table_at {
    ($root:expr, $($key:ident).+) => {
        crate::loader::lua_util::table_path($root, &[$(stringify!($key)),+])
    };
}

pub(crate) use table_at;

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
pub(crate) fn table_path(root: &Table, path: &[&str]) -> Option<Table> {
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
                    error = mineral_log::chain(color_eyre::Report::new(e)),
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

    /// 点号宏全路径命中;缺失键与非表节点都静默 `None`。
    #[test]
    fn walks_path_and_misses_quietly() -> color_eyre::Result<()> {
        let lua = Lua::new();
        let root: mlua::Table = lua
            .load(r#"{ tui = { copy = { templates = { 1 } } }, flat = 5 }"#)
            .eval()?;
        let hit = table_at!(&root, tui.copy.templates);
        assert_eq!(hit.map(|t| t.raw_len()), Some(1), "全路径命中");
        assert!(table_at!(&root, tui.missing).is_none(), "缺失键静默 None");
        assert!(
            table_at!(&root, flat.deeper).is_none(),
            "非表节点 None(仅 debug 日志)"
        );
        Ok(())
    }
}

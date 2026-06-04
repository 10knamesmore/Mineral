//! 调用内置 `merge.lua` 做深合并的胶水。

use mlua::{Function, Lua, Table};

/// 调用内置 `merge.lua`:`merged = deep_merge(default, user)`(数组整体替换)。
/// 返回新表,不改动 `default` / `user`。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `default`: 默认表
///   - `user`: 用户表
///
/// # Return:
///   合并后的新表
pub(crate) fn deep_merge(lua: &Lua, default: Table, user: Table) -> color_eyre::Result<Table> {
    let merge_fn: Function = lua
        .load(include_str!("../lua/merge.lua"))
        .set_name("merge.lua")
        .eval()?;
    let merged: Table = merge_fn.call((default, user))?;
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use mlua::{Lua, LuaSerdeExt, Table, Value};
    use proptest::prelude::{Strategy, any, prop_oneof, proptest};

    use super::deep_merge;

    /// eval 一段返回表的 Lua 源,取出表(测试辅助)。
    fn eval_table(lua: &Lua, src: &str) -> color_eyre::Result<Table> {
        let table: Table = lua.load(src).eval()?;
        Ok(table)
    }

    /// 把 Lua 表序列化回 `serde_json::Value`(便于结构比较)。
    fn table_to_json(table: Table) -> color_eyre::Result<serde_json::Value> {
        let json = serde_json::to_value(Value::Table(table))?;
        Ok(json)
    }

    #[test]
    fn replaces_arrays_wholesale() -> color_eyre::Result<()> {
        let lua = Lua::new();
        let base = eval_table(&lua, r#"return { k = { "a", "b", "c" } }"#)?;
        let over = eval_table(&lua, r#"return { k = { "x" } }"#)?;
        let merged = table_to_json(deep_merge(&lua, base, over)?)?;
        assert_eq!(
            merged,
            serde_json::json!({ "k": ["x"] }),
            "数组整体替换非追加"
        );
        Ok(())
    }

    #[test]
    fn deep_merges_preserving_siblings() -> color_eyre::Result<()> {
        let lua = Lua::new();
        let base = eval_table(&lua, r#"return { m = { a = 1, b = 2 } }"#)?;
        let over = eval_table(&lua, r#"return { m = { b = 9 } }"#)?;
        let merged = table_to_json(deep_merge(&lua, base, over)?)?;
        assert_eq!(
            merged,
            serde_json::json!({ "m": { "a": 1, "b": 9 } }),
            "深合并:覆盖 b、保留 a"
        );
        Ok(())
    }

    #[test]
    fn empty_override_is_identity() -> color_eyre::Result<()> {
        let lua = Lua::new();
        let base = eval_table(&lua, r#"return { a = 1, m = { b = 2 } }"#)?;
        let over = eval_table(&lua, "return {}")?;
        let merged = table_to_json(deep_merge(&lua, base, over)?)?;
        assert_eq!(merged, serde_json::json!({ "a": 1, "m": { "b": 2 } }));
        Ok(())
    }

    /// 任意嵌套 map(值为整数 / 短字符串 / 子 map),供不变量测试。
    fn arb_json() -> impl Strategy<Value = serde_json::Value> {
        let leaf = prop_oneof![
            any::<i32>().prop_map(|n| serde_json::json!(n)),
            "[a-z]{1,4}".prop_map(serde_json::Value::String),
        ];
        leaf.prop_recursive(3, 32, 4, |inner| {
            proptest::collection::btree_map("[a-z]{1,3}", inner, 0..4)
                .prop_map(|m| serde_json::Value::Object(m.into_iter().collect()))
        })
    }

    /// 顶层必为 map(merge 要求两侧 map)。
    fn arb_object() -> impl Strategy<Value = serde_json::Value> {
        proptest::collection::btree_map("[a-z]{1,3}", arb_json(), 0..4)
            .prop_map(|m| serde_json::Value::Object(m.into_iter().collect()))
    }

    /// 经 Lua 往返合并 `base` 与 `over`,返回合并结果 JSON。
    fn roundtrip_merge(
        base: &serde_json::Value,
        over: &serde_json::Value,
    ) -> color_eyre::Result<serde_json::Value> {
        let lua = Lua::new();
        let (Value::Table(base_t), Value::Table(over_t)) =
            (lua.to_value(base)?, lua.to_value(over)?)
        else {
            return Err(color_eyre::eyre::eyre!("生成的不是表"));
        };
        table_to_json(deep_merge(&lua, base_t, over_t)?)
    }

    proptest! {
        /// merge(d, {}) == d:空 override 不改变 base。
        #[test]
        fn merge_with_empty_is_identity(base in arb_object()) {
            let got = roundtrip_merge(&base, &serde_json::json!({}));
            proptest::prop_assert!(got.is_ok(), "merge 失败:{got:?}");
            if let Ok(merged) = got {
                proptest::prop_assert_eq!(merged, base);
            }
        }

        /// merge(d, d) == d:幂等。
        #[test]
        fn merge_is_idempotent(base in arb_object()) {
            let got = roundtrip_merge(&base, &base);
            proptest::prop_assert!(got.is_ok(), "merge 失败:{got:?}");
            if let Ok(merged) = got {
                proptest::prop_assert_eq!(merged, base);
            }
        }
    }
}

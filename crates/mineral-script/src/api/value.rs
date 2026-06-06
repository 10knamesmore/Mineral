//! [`PropValue`] → Lua 值的单向转换(observe 回放 / `mineral.get` /
//! `PropertyChanged` 分发共用)。

use mlua::{IntoLua, Lua};

use crate::message::PropValue;

/// 把属性值转成 Lua 值:`Int` → integer、`Str` → string、`None` → nil。
///
/// # Params:
///   - `lua`: 目标 VM(string 需要在 VM 里分配)
///   - `value`: 属性值
///
/// # Return:
///   对应的 Lua 值;VM 分配失败时为 `Err`。
pub(crate) fn prop_to_lua(lua: &Lua, value: &PropValue) -> mlua::Result<mlua::Value> {
    match value {
        PropValue::Int(n) => (*n).into_lua(lua),
        PropValue::Str(s) => s.as_str().into_lua(lua),
        PropValue::None => Ok(mlua::Value::Nil),
    }
}

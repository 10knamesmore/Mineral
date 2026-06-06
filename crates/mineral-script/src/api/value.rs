//! Lua 值 ↔ 结构化 Rust 类型的共享转换:属性值、歌曲 id、store 标量。
//! 各 API 文件(player / store / observe / queue / library)共用,
//! 出了 api 模块全是强类型。

use mineral_model::{SongId, SourceKind};
use mineral_protocol::StoreValue;
use mlua::{IntoLua, Lua};

use crate::message::{PropKey, PropValue};

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

/// 解析 qualified 形式的歌曲 id(`"namespace:value"`,即事件回调里
/// `args.song.id` 给出的格式)。
///
/// # Params:
///   - `raw`: 脚本侧输入
///
/// # Return:
///   结构化 [`SongId`];缺冒号 / 两段有空者报 Lua 错。
pub(crate) fn parse_song_id(raw: &str) -> mlua::Result<SongId> {
    let bad = || {
        mlua::Error::RuntimeError(format!(
            "invalid song id {raw:?}, expected \"namespace:value\" (e.g. \"netease:123\")"
        ))
    };
    let (namespace, value) = raw.split_once(':').ok_or_else(bad)?;
    if namespace.is_empty() || value.is_empty() {
        return Err(bad());
    }
    // namespace 开放(插件源),未知名经 intern 铸造 —— 与模型层哲学一致。
    Ok(SongId::new(SourceKind::from_name(namespace), value))
}

/// 解析属性名;未知名报 Lua 错并列出全部合法名(`observe` / `get` 共用)。
///
/// # Params:
///   - `prop`: 脚本侧输入
pub(crate) fn parse_prop(prop: &str) -> mlua::Result<PropKey> {
    PropKey::from_name(prop).ok_or_else(|| {
        let expected = PropKey::ALL.map(PropKey::as_str).join("\" | \"");
        mlua::Error::RuntimeError(format!(
            "unknown property {prop:?}, expected \"{expected}\""
        ))
    })
}

/// 解析 qualified 形式的歌单 id(`"namespace:value"`,`library.tracks` 用)。
///
/// # Params:
///   - `raw`: 脚本侧输入
pub(crate) fn parse_playlist_id(raw: &str) -> mlua::Result<mineral_model::PlaylistId> {
    let bad = || {
        mlua::Error::RuntimeError(format!(
            "invalid playlist id {raw:?}, expected \"namespace:value\" (e.g. \"netease:123\")"
        ))
    };
    let (namespace, value) = raw.split_once(':').ok_or_else(bad)?;
    if namespace.is_empty() || value.is_empty() {
        return Err(bad());
    }
    Ok(mineral_model::PlaylistId::new(
        SourceKind::from_name(namespace),
        value,
    ))
}

/// 开放 store key 推荐带 `.` 前缀(如 `plugin.skipcount`):与未来一等字段名
/// 隔开命名空间。无前缀只 warn 不拒(渐进约定;`store.set` / `store.inc` 共用)。
///
/// # Params:
///   - `key`: 脚本侧输入的开放键
pub(crate) fn warn_unprefixed_key(key: &str) {
    if !key.contains('.') {
        mineral_log::warn!(
            target: "script",
            key,
            "store key 建议带 `.` 前缀(如 \"plugin.{key}\"),避免与未来一等字段冲突"
        );
    }
}

/// Lua 标量 → [`StoreValue`](`store.set` 的入参转换)。
///
/// 整数 number → `Int`,其余 number → `Real`;不支持 table / function 等
/// 复合类型,报 Lua 错。
///
/// # Params:
///   - `value`: 脚本侧输入
pub(crate) fn lua_to_store(value: &mlua::Value) -> mlua::Result<StoreValue> {
    match value {
        mlua::Value::Nil => Ok(StoreValue::Nil),
        mlua::Value::Boolean(b) => Ok(StoreValue::Bool(*b)),
        mlua::Value::Integer(n) => Ok(StoreValue::Int(*n)),
        mlua::Value::Number(f) => Ok(StoreValue::Real(*f)),
        mlua::Value::String(s) => Ok(StoreValue::Text(s.to_str()?.to_owned())),
        other => Err(mlua::Error::RuntimeError(format!(
            "store value must be a scalar (nil/boolean/number/string), got {}",
            other.type_name()
        ))),
    }
}

/// [`StoreValue`] → Lua 值(`store.get` / `store.inc` 回调实参)。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `value`: 持久值
pub(crate) fn store_to_lua(lua: &Lua, value: &StoreValue) -> mlua::Result<mlua::Value> {
    match value {
        StoreValue::Int(n) => (*n).into_lua(lua),
        StoreValue::Real(f) => (*f).into_lua(lua),
        StoreValue::Text(s) => s.as_str().into_lua(lua),
        StoreValue::Bool(b) => Ok(mlua::Value::Boolean(*b)),
        StoreValue::Nil => Ok(mlua::Value::Nil),
    }
}

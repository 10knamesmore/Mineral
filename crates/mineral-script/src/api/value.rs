//! 音乐实体 id、per-song store 标量与配置覆盖值的共享 Lua 转换。

use mineral_model::{SongId, SourceKind};
use mineral_protocol::StoreValue;
use mlua::{IntoLua, Lua};

/// 解析 qualified 形式的歌曲 id(`"namespace:value"`)，与音乐 hook 和查询投影一致。
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

/// 开放 store key 推荐使用带 `.` 分隔的命名空间前缀(如 `plugin.skipcount`)。
/// `store.set` 遇到未分隔的命名空间只 warn，不拒绝。
///
/// # Params:
///   - `key`: 脚本侧输入的开放键
pub(crate) fn warn_unprefixed_key(key: &str) {
    if !key.contains('.') {
        mineral_log::warn!(
            target: "script",
            key,
            "store key 建议使用命名空间前缀(如 \"plugin.{key}\"),隔离插件数据"
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

/// [`StoreValue`] → Lua 值(`store.get` 回调实参)。
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

/// 配置值的嵌套深度上限，循环引用表在边界报错。
const CONFIG_MAX_DEPTH: u8 = 8;

/// Lua 配置值 → [`mineral_protocol::BusValue`]，由两侧配置覆盖共用。
///
/// table 判形:键全为 `1..=n` 连续整数 → `Array`;键全为字符串 → `Map`
/// (保留遍历顺序);混合 / 其他键型报错。function / userdata 等不可
/// 序列化类型报错。
///
/// # Params:
///   - `value`: 脚本侧载荷
///   - `depth`: 当前递归深度(顶层传 0)
pub(crate) fn lua_to_bus(
    value: &mlua::Value,
    depth: u8,
) -> mlua::Result<mineral_protocol::BusValue> {
    use mineral_protocol::BusValue;
    if depth > CONFIG_MAX_DEPTH {
        return Err(mlua::Error::RuntimeError(format!(
            "配置值嵌套超过 {CONFIG_MAX_DEPTH} 层(循环引用?)"
        )));
    }
    match value {
        mlua::Value::Nil => Ok(BusValue::Nil),
        mlua::Value::Boolean(b) => Ok(BusValue::Bool(*b)),
        mlua::Value::Integer(n) => Ok(BusValue::Int(*n)),
        mlua::Value::Number(f) => Ok(BusValue::Float(*f)),
        mlua::Value::String(s) => Ok(BusValue::Str(s.to_str()?.to_owned())),
        mlua::Value::Table(table) => lua_table_to_bus(table, depth),
        other => Err(mlua::Error::RuntimeError(format!(
            "payload 不支持 {} 类型(可用:nil/boolean/number/string/table)",
            other.type_name()
        ))),
    }
}

/// [`lua_to_bus`] 的 table 分支:数组 / 映射判形与逐项递归。
fn lua_table_to_bus(table: &mlua::Table, depth: u8) -> mlua::Result<mineral_protocol::BusValue> {
    use mineral_protocol::BusValue;
    let mut ints = Vec::<(i64, BusValue)>::new();
    let mut strs = Vec::<(String, BusValue)>::new();
    for pair in table.clone().pairs::<mlua::Value, mlua::Value>() {
        let (key, item) = pair?;
        let item = lua_to_bus(&item, depth.saturating_add(1))?;
        match key {
            mlua::Value::Integer(i) => ints.push((i, item)),
            mlua::Value::String(s) => strs.push((s.to_str()?.to_owned(), item)),
            other => {
                return Err(mlua::Error::RuntimeError(format!(
                    "payload 的 table 键须是字符串或整数,实得 {}",
                    other.type_name()
                )));
            }
        }
    }
    match (ints.is_empty(), strs.is_empty()) {
        // 空表无法判定数组形，按空配置映射处理。
        (true, true) => Ok(BusValue::Map(Vec::new())),
        (false, true) => {
            ints.sort_unstable_by_key(|&(i, _)| i);
            let consecutive = ints
                .iter()
                .enumerate()
                .all(|(idx, &(i, _))| i64::try_from(idx.wrapping_add(1)) == Ok(i));
            if !consecutive {
                return Err(mlua::Error::runtime(
                    "payload 的整数键须是 1..=n 连续数组形",
                ));
            }
            Ok(BusValue::Array(
                ints.into_iter().map(|(_, item)| item).collect(),
            ))
        }
        (true, false) => Ok(BusValue::Map(strs)),
        (false, false) => Err(mlua::Error::runtime(
            "payload 的 table 不能混用数组与字符串键",
        )),
    }
}

/// 配置覆盖值 → Lua 值，仅用于验证解析往返。
///
/// # Params:
///   - `lua`: 目标 VM
///   - `value`: 配置覆盖值
#[cfg(test)]
pub(crate) fn bus_to_lua(
    lua: &Lua,
    value: &mineral_protocol::BusValue,
) -> mlua::Result<mlua::Value> {
    use mineral_protocol::BusValue;
    match value {
        BusValue::Nil => Ok(mlua::Value::Nil),
        BusValue::Bool(b) => Ok(mlua::Value::Boolean(*b)),
        BusValue::Int(n) => (*n).into_lua(lua),
        BusValue::Float(f) => (*f).into_lua(lua),
        BusValue::Str(s) => s.as_str().into_lua(lua),
        BusValue::Array(items) => {
            let table = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                table.set(i.wrapping_add(1), bus_to_lua(lua, item)?)?;
            }
            Ok(mlua::Value::Table(table))
        }
        BusValue::Map(entries) => {
            let table = lua.create_table()?;
            for (key, item) in entries {
                table.set(key.as_str(), bus_to_lua(lua, item)?)?;
            }
            Ok(mlua::Value::Table(table))
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::BusValue;
    use proptest::prelude::{Strategy, prop_oneof, proptest};

    use super::{bus_to_lua, lua_to_bus};

    /// 任意配置覆盖值(深度 ≤3;数组 / 映射非空——空 table 在 Lua 形不可辨,
    /// 单独用例覆盖)。浮点取有限值(NaN 无自反等价,round-trip 断言无意义);
    /// **容器内不出 Nil**:Lua 的 `{nil}` / `{k=nil}` 即键缺席,语义上
    /// 不可保真，Nil 只在顶层标量位成立。
    fn arb_bus() -> impl Strategy<Value = BusValue> {
        let leaf = prop_oneof![
            proptest::prelude::any::<bool>().prop_map(BusValue::Bool),
            proptest::prelude::any::<i64>().prop_map(BusValue::Int),
            (-1.0e12_f64..1.0e12).prop_map(BusValue::Float),
            "[a-z0-9央钠]{0,8}".prop_map(BusValue::Str),
        ];
        leaf.prop_recursive(
            /*depth*/ 3,
            /*desired_size*/ 24,
            /*expected_branch_size*/ 4,
            |inner| {
                prop_oneof![
                    proptest::collection::vec(inner.clone(), 1..4).prop_map(BusValue::Array),
                    proptest::collection::vec(("[a-z]{1,6}", inner), 1..4).prop_map(|pairs| {
                        // 键去重:Lua table 同键互踩,round-trip 无从保真。
                        let mut seen = rustc_hash::FxHashSet::default();
                        BusValue::Map(
                            pairs
                                .into_iter()
                                .filter(|(k, _)| seen.insert(k.clone()))
                                .collect(),
                        )
                    }),
                ]
            },
        )
    }

    /// 排序 Map 键做规范形比较:Lua table 不保序,round-trip 只保集合等价。
    fn canonical(value: &BusValue) -> BusValue {
        match value {
            BusValue::Array(items) => BusValue::Array(items.iter().map(canonical).collect()),
            BusValue::Map(entries) => {
                let mut sorted = entries
                    .iter()
                    .map(|(k, v)| (k.clone(), canonical(v)))
                    .collect::<Vec<(String, BusValue)>>();
                sorted.sort_by(|a, b| a.0.cmp(&b.0));
                BusValue::Map(sorted)
            }
            other => other.clone(),
        }
    }

    proptest! {
        /// 不变量:任意载荷经 BusValue → Lua → BusValue 集合等价(Map 不保序)。
        #[test]
        fn bus_value_roundtrips_through_lua(value in arb_bus()) {
            let lua = mlua::Lua::new();
            let mid = bus_to_lua(&lua, &value)?;
            let back = lua_to_bus(&mid, /*depth*/ 0)?;
            proptest::prop_assert_eq!(canonical(&back), canonical(&value));
        }
    }

    #[test]
    fn nil_roundtrips_as_top_level_scalar() -> color_eyre::Result<()> {
        let lua = mlua::Lua::new();
        let mid = bus_to_lua(&lua, &BusValue::Nil)?;
        assert_eq!(lua_to_bus(&mid, /*depth*/ 0)?, BusValue::Nil);
        Ok(())
    }

    #[test]
    fn empty_table_maps_to_empty_map() -> color_eyre::Result<()> {
        let lua = mlua::Lua::new();
        let empty = lua.create_table()?;
        assert_eq!(
            lua_to_bus(&mlua::Value::Table(empty), /*depth*/ 0)?,
            BusValue::Map(Vec::new()),
            "空 table 形不可辨,统一按空 Map"
        );
        Ok(())
    }

    #[test]
    fn mixed_keys_and_functions_are_rejected() -> color_eyre::Result<()> {
        let lua = mlua::Lua::new();
        let mixed = lua.create_table()?;
        mixed.set(1, "a")?;
        mixed.set("k", "b")?;
        assert!(
            lua_to_bus(&mlua::Value::Table(mixed), /*depth*/ 0).is_err(),
            "同层混用数组/字符串键必须报错"
        );
        let func = lua.create_function(|_, ()| Ok(()))?;
        assert!(
            lua_to_bus(&mlua::Value::Function(func), /*depth*/ 0).is_err(),
            "function 载荷必须报错"
        );
        Ok(())
    }
}

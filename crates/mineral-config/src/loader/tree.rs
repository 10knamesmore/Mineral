//! 有效配置树:JSON 值层面的深合并 / 点分路径嵌套 / 落型。
//!
//! 供 session 级覆盖(overlay)在**不重 eval Lua** 的前提下叠加到合成树上:
//! 合并语义与 `lua/merge.lua` 逐条对齐(map 递归、数组整体替换、类型不一
//! 整体替换),一致性由共享向量测试钉住(同一组向量分别过两个实现)。

use crate::loader::warning::ConfigWarning;
use crate::schema::Config;

/// 深合并两棵 JSON 树:`overlay` 的键覆盖 `base`。
///
/// 两侧同键且都是 Object → 递归合并;否则(标量 / 数组 / 类型不一)
/// overlay 整体替换——与 `lua/merge.lua` 同语义。
///
/// # Params:
///   - `base`: 底树(通常是 default+user 的合成树)
///   - `overlay`: 覆盖树
///
/// # Return:
///   合并后的新树
pub fn merge_tree(base: serde_json::Value, overlay: serde_json::Value) -> serde_json::Value {
    match (base, overlay) {
        (serde_json::Value::Object(mut base_map), serde_json::Value::Object(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                let merged = match base_map.remove(&key) {
                    Some(base_value) => merge_tree(base_value, overlay_value),
                    None => overlay_value,
                };
                base_map.insert(key, merged);
            }
            serde_json::Value::Object(base_map)
        }
        (_, overlay) => overlay,
    }
}

/// 把点分路径折成嵌套单键树:`"tui.lyrics.gap"` + `4` →
/// `{"tui":{"lyrics":{"gap":4}}}`,供逐条 overlay 经 [`merge_tree`] 叠加。
///
/// # Params:
///   - `path`: 点分配置路径(非空;段不校验,拼错的段由落型报 unknown field)
///   - `value`: 叶子值
///
/// # Return:
///   嵌套单键树
pub fn nest_path(path: &str, value: serde_json::Value) -> serde_json::Value {
    path.rsplit('.').fold(value, |acc, segment| {
        let mut map = serde_json::Map::new();
        map.insert(segment.to_owned(), acc);
        serde_json::Value::Object(map)
    })
}

/// 把配置树落成强类型 [`Config`](经 `serde_path_to_error` 报精确字段路径)。
///
/// # Params:
///   - `tree`: 配置树(须是完整合成树,不是增量片段)
///
/// # Return:
///   `Config`;失败给带路径的 [`ConfigWarning::Deserialize`](供 overlay 剔除定位)
pub fn from_tree(tree: &serde_json::Value) -> Result<Config, ConfigWarning> {
    serde_path_to_error::deserialize::<_, Config>(tree).map_err(|e| ConfigWarning::Deserialize {
        path: e.path().to_string(),
        detail: e.inner().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{from_tree, merge_tree, nest_path};
    use crate::loader::warning::ConfigWarning;

    /// 合并语义共享向量:同一组 (base, overlay, expected) 同时过 Rust 实现与
    /// `merge.lua`,两边结果必须一致——防两份实现漂移。
    #[test]
    fn merge_semantics_match_lua_implementation() -> color_eyre::Result<()> {
        let vectors = [
            // map 递归:同键子 map 逐键合并。
            (
                json!({"a": {"x": 1, "y": 2}}),
                json!({"a": {"y": 3}}),
                json!({"a": {"x": 1, "y": 3}}),
            ),
            // 数组整体替换,不逐元素合并。
            (
                json!({"arr": [1, 2, 3]}),
                json!({"arr": [9]}),
                json!({"arr": [9]}),
            ),
            // 类型不一:overlay 整体替换(map → 标量)。
            (json!({"v": {"x": 1}}), json!({"v": 5}), json!({"v": 5})),
            // 标量 → map 同样整体替换。
            (
                json!({"v": 5}),
                json!({"v": {"x": 1}}),
                json!({"v": {"x": 1}}),
            ),
            // 标量覆盖 + 新键并入。
            (
                json!({"a": 1, "keep": true}),
                json!({"a": 2, "b": "new"}),
                json!({"a": 2, "b": "new", "keep": true}),
            ),
            // 空 map overlay = 不改变(merge(d, {}) == d)。
            (
                json!({"a": {"x": 1}}),
                json!({"a": {}}),
                json!({"a": {"x": 1}}),
            ),
            // 深层嵌套递归。
            (
                json!({"t": {"lyrics": {"gap": 1, "ms": 280}}}),
                json!({"t": {"lyrics": {"gap": 4}}}),
                json!({"t": {"lyrics": {"gap": 4, "ms": 280}}}),
            ),
        ];
        let lua = mlua::Lua::new();
        for (base, overlay, expected) in vectors {
            let rust_merged = merge_tree(base.clone(), overlay.clone());
            assert_eq!(
                rust_merged, expected,
                "Rust 合并结果不符:base={base} overlay={overlay}"
            );
            let lua_merged = lua_merge(&lua, &base, &overlay)?;
            assert_eq!(
                lua_merged, expected,
                "merge.lua 合并结果不符:base={base} overlay={overlay}"
            );
        }
        Ok(())
    }

    /// 把一组 JSON 树喂给 `merge.lua` 实现,返回合并结果(JSON 表示)。
    ///
    /// # Params:
    ///   - `lua`: 测试 VM
    ///   - `base` / `overlay`: 待合并两树
    ///
    /// # Return:
    ///   合并结果转回 JSON
    fn lua_merge(
        lua: &mlua::Lua,
        base: &serde_json::Value,
        overlay: &serde_json::Value,
    ) -> color_eyre::Result<serde_json::Value> {
        use mlua::LuaSerdeExt;
        let base_table = match lua.to_value(base)? {
            mlua::Value::Table(t) => t,
            other => color_eyre::eyre::bail!("base 应转成表,得到 {other:?}"),
        };
        let overlay_table = match lua.to_value(overlay)? {
            mlua::Value::Table(t) => t,
            other => color_eyre::eyre::bail!("overlay 应转成表,得到 {other:?}"),
        };
        let merged = crate::loader::merge::deep_merge(lua, base_table, overlay_table)?;
        Ok(serde_json::to_value(mlua::Value::Table(merged))?)
    }

    /// 点分路径折成嵌套单键树:`"tui.lyrics.gap"` + 4 → `{"tui":{"lyrics":{"gap":4}}}`。
    #[test]
    fn nest_path_builds_singleton_tree() {
        assert_eq!(
            nest_path("tui.lyrics.gap", json!(4)),
            json!({"tui": {"lyrics": {"gap": 4}}})
        );
        assert_eq!(nest_path("volume", json!(50)), json!({"volume": 50}));
    }

    /// 落型:合法树成 Config;非法值报精确字段路径(供 overlay 剔除定位)。
    #[test]
    fn from_tree_types_and_reports_precise_path() -> color_eyre::Result<()> {
        // 默认树(无用户文件的 load_with_vm 产物)必落型成功。
        let absent = std::env::temp_dir().join("mineral-cfg-tree-absent.lua");
        let loaded = crate::loader::pipeline::load_with_vm(&absent, |_lua| Ok(()))?;
        let cfg = from_tree(&loaded.tree).map_err(|w| color_eyre::eyre::eyre!("{w}"))?;
        assert_eq!(*cfg.audio().volume(), 100);
        // 坏值:路径精确到字段。
        let mut bad = loaded.tree.clone();
        if let Some(v) = bad.pointer_mut("/audio/volume") {
            *v = serde_json::Value::String("loud".to_owned());
        }
        match from_tree(&bad) {
            Err(ConfigWarning::Deserialize { path, .. }) => {
                assert_eq!(path, "audio.volume", "路径应精确到字段");
            }
            other => color_eyre::eyre::bail!("应报 Deserialize 告警,得到 {other:?}"),
        }
        Ok(())
    }
}

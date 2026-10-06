//! TUI 默认值、文件校验和本地 session 数据树的落型入口。

use std::path::Path;

use mineral_config::{ConfigWarning, Result};
use mineral_script::mlua::Lua;

use super::TuiConfig;
use super::callbacks::prepare_tui;

/// TUI 默认配置的唯一来源。
pub(super) const DEFAULT: &str = include_str!("lua/tui-default.lua");

/// 校验指定 tui.lua,不执行 setup,也不读取 daemon.lua。
pub fn load_tui(user_path: &Path) -> Result<(TuiConfig, Vec<ConfigWarning>)> {
    let lua = Lua::new();
    let loaded =
        mineral_config::load_file(&lua, user_path, DEFAULT, "tui-default.lua", prepare_tui)?;
    Ok((loaded.config, loaded.warnings))
}

/// 在本地脚本 VM 中一次求值 tui.lua 并落型;失败不回落成成功重载。
pub(super) fn tui_from_source(
    lua: &Lua,
    source: &str,
    name: &str,
) -> Result<(TuiConfig, serde_json::Value)> {
    mineral_config::from_source(lua, source, name, DEFAULT, "tui-default.lua", prepare_tui)
}

impl TuiConfig {
    /// 只求值内置 TUI 默认;不加载 daemon 默认或任何用户文件。
    pub fn defaults() -> Result<Self> {
        let (config, _) = mineral_config::defaults(DEFAULT, "tui-default.lua", prepare_tui)?;
        Ok(config)
    }
}

/// 本地默认树;不含 daemon 设置或脚本函数。
pub(crate) fn default_tui_tree() -> Result<serde_json::Value> {
    let (_, tree) = mineral_config::defaults::<TuiConfig>(DEFAULT, "tui-default.lua", prepare_tui)?;
    Ok(tree)
}

/// 校验完整 TUI 数据树;拒绝 daemon 字段和旧根包装。
pub(crate) fn tui_from_tree(
    tree: &serde_json::Value,
) -> std::result::Result<TuiConfig, ConfigWarning> {
    mineral_config::deserialize_tree(tree)
}

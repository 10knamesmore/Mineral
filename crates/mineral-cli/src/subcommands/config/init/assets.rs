//! CLI 协调两个宿主写出用户配置和完整 LuaLS 资产。

use std::path::Path;

use mineral_config::{InitOutcome, Result, create_dir, overwrite, recreate_dir, write_if_absent};

/// 写出两份宿主配置;保护用户文件并清理过期生成类型。
/// 同步文件操作由 config 命令的阻塞任务调用。
pub(crate) fn run_init(config_dir: &Path) -> Result<Vec<InitOutcome>> {
    create_dir(config_dir)?;
    let meta = config_dir.join("lua/meta");
    recreate_dir(&meta)?;
    let mut outcomes = vec![write_if_absent(
        &config_dir.join(".luarc.json"),
        include_str!("../lua/luarc.json"),
    )?];
    outcomes.extend(mineral_server::config::init(config_dir)?);
    outcomes.extend(mineral_tui::config::init(config_dir)?);
    outcomes.push(overwrite(
        &meta.join("types.lua"),
        mineral_script::TYPES_META,
    )?);
    mineral_log::info!(target: "config", path = %config_dir.display(), assets = outcomes.len(), "daemon and TUI configuration assets initialized");
    Ok(outcomes)
}

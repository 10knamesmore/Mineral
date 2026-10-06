//! daemon 默认值、文件加载和 session 数据树的落型入口。

use std::path::Path;

use mineral_config::{ConfigWarning, Error, Result};
use mineral_script::mlua::Lua;

use super::DaemonConfig;
use super::callbacks::prepare_daemon;

/// daemon 默认配置的唯一来源。
pub(super) const DEFAULT: &str = include_str!("lua/daemon-default.lua");

/// daemon 文件加载结果;失败只返回默认和诊断,不返回待激活 VM。
pub struct DaemonLoad {
    /// 已校验的 daemon 配置,不含 TUI 设置。
    pub config: DaemonConfig,

    /// 用户文件读取、求值或落型失败的诊断。
    pub warnings: Vec<ConfigWarning>,

    /// 成功加载用户文件的 VM;setup 已摘取但未执行。
    pub vm: Option<Lua>,

    /// 默认与用户表合成的数据树,用于本进程 session 覆盖。
    pub tree: serde_json::Value,
}

/// 校验指定 daemon.lua,不执行 setup;缺失或失败使用 daemon 默认。
pub fn load_daemon(user_path: &Path) -> Result<(DaemonConfig, Vec<ConfigWarning>)> {
    let lua = Lua::new();
    let loaded = mineral_config::load_file(
        &lua,
        user_path,
        DEFAULT,
        "daemon-default.lua",
        prepare_daemon,
    )?;
    Ok((loaded.config, loaded.warnings))
}

/// 安装音乐 API 后加载 daemon.lua;调用方决定成功后如何执行 setup。
/// 缺失或失败不交还 VM,不会影响已有脚本线程。
pub fn load_daemon_with_vm(
    user_path: &Path,
    install: impl FnOnce(&Lua) -> mineral_script::mlua::Result<()>,
) -> Result<DaemonLoad> {
    let lua = Lua::new();
    install(&lua).map_err(|source| Error::Lua {
        operation: "安装 daemon 脚本 API",
        source,
    })?;
    let loaded = mineral_config::load_file(
        &lua,
        user_path,
        DEFAULT,
        "daemon-default.lua",
        prepare_daemon,
    )?;
    Ok(DaemonLoad {
        config: loaded.config,
        warnings: loaded.warnings,
        vm: loaded.user_loaded.then_some(lua),
        tree: loaded.tree,
    })
}

impl DaemonConfig {
    /// 只求值内置 daemon 默认;程序分发的默认配置损坏时返回错误。
    pub fn defaults() -> Result<Self> {
        let (config, _) = mineral_config::defaults(DEFAULT, "daemon-default.lua", prepare_daemon)?;
        Ok(config)
    }
}

/// 内置 daemon 默认树;供宿主测试构造 session 覆盖,不读取用户文件。
#[cfg(test)]
pub(crate) fn default_daemon_tree() -> Result<serde_json::Value> {
    let (_, tree) =
        mineral_config::defaults::<DaemonConfig>(DEFAULT, "daemon-default.lua", prepare_daemon)?;
    Ok(tree)
}

/// 校验完整 daemon 数据树;拒绝 TUI 字段和旧根包装。
pub(crate) fn daemon_from_tree(
    tree: &serde_json::Value,
) -> std::result::Result<DaemonConfig, ConfigWarning> {
    mineral_config::deserialize_tree(tree)
}

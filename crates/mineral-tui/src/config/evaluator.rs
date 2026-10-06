//! 一次求值 TUI 配置并执行 setup;成功后同时交还配置、回调 VM 和效果。

use mineral_script::{TuiCommand, TuiRuntime, WatchdogConfig};

use super::TuiConfig;
use super::loader::tui_from_source;

/// 本地配置求值或脚本执行失败;未成功的 VM 和命令不进入 App。
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// 源码求值、合并或配置落型失败。
    #[error("TUI 配置加载失败")]
    Config(#[from] mineral_config::Error),

    /// API 安装或 setup 执行失败。
    #[error("TUI 脚本执行失败")]
    Script(#[from] mineral_script::Error),
}

/// 已加载且 setup 成功的配置;App 校验覆盖后一次性激活。
#[derive(Debug)]
pub(crate) struct TuiLoad {
    /// 本地文件落型后的根配置。
    pub(crate) config: TuiConfig,

    /// 与配置一致的数据树;setup 和模板函数已摘取。
    pub(crate) tree: serde_json::Value,

    /// 持有同次求值的复制模板及其闭包状态。
    pub(crate) runtime: TuiRuntime,

    /// 顶层求值和 setup 暂存的效果,尚未提交到 App。
    pub(crate) commands: Vec<TuiCommand>,
}

/// 在预算保护下求值一次 tui.lua;setup 和复制模板使用新配置的预算。
pub(crate) fn evaluate_tui(
    source: &str,
    name: &str,
    watchdog: WatchdogConfig,
) -> Result<TuiLoad, Error> {
    let loaded = mineral_script::evaluate_tui(name, watchdog, |lua| {
        let (config, tree) = tui_from_source(lua, source, name)?;
        let watchdog = WatchdogConfig::from(config.script());
        Ok::<_, Error>(((config, tree), watchdog))
    })?;
    let (config, tree) = loaded.value;
    Ok(TuiLoad {
        config,
        tree,
        runtime: loaded.runtime,
        commands: loaded.commands,
    })
}

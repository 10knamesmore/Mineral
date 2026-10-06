//! 在本地 VM 中暂存 TUI API 效果;宿主负责配置求值和预算转换。

use mlua::Lua;

use super::{TuiCommand, TuiHost, TuiRuntime};
use crate::Error;
use crate::api;
use crate::watchdog::{WatchdogConfig, with_watchdog};

/// 一次成功加载的宿主数据、本地 VM 与有序效果;失败不交还部分产物。
#[derive(Debug)]
pub struct TuiLoad<T> {
    /// TUI 求值并校验后的数据;脚本层不解释其配置结构。
    pub value: T,

    /// 保留复制模板闭包的本地 VM,不创建线程。
    pub runtime: TuiRuntime,

    /// 顶层求值与 setup 产生的效果,由宿主统一校验后提交。
    pub commands: Vec<TuiCommand>,
}

/// 安装 TUI 独有的模块,音乐 API 不进入这个 VM。
fn install_tui_api(lua: &Lua, host: &TuiHost) -> mlua::Result<()> {
    let mineral = lua.create_table()?;
    api::log::install(lua, &mineral)?;
    api::sys::install(lua, &mineral)?;
    api::ui::install(lua, &mineral, host)?;
    let collector = host.clone();
    api::config::install(lua, &mineral, move |ops| {
        collector.push(TuiCommand::ConfigOverride { ops });
    })?;
    api::install_module(lua, "mineral.tui", mineral)
}

/// 安装 TUI API,保护宿主的一次求值,再执行同 VM 中摘取的 setup(api)。
/// evaluate 返回已校验的数据和后续预算;任一步失败都丢弃新 VM 与效果。
/// name 仅用于日志定位,本函数不读取配置文件或提供配置默认值。
pub fn evaluate_tui<T, E>(
    name: &str,
    watchdog: WatchdogConfig,
    evaluate: impl FnOnce(&Lua) -> std::result::Result<(T, WatchdogConfig), E>,
) -> std::result::Result<TuiLoad<T>, E>
where
    E: std::error::Error + From<Error> + 'static,
{
    let lua = Lua::new();
    let host = TuiHost::default();
    let evaluated = (|| -> std::result::Result<_, E> {
        install_tui_api(&lua, &host).map_err(|source| Error::Lua {
            operation: "安装 TUI API",
            source,
        })?;
        let (value, watchdog) = with_watchdog(&lua, &watchdog, || evaluate(&lua))?;
        crate::setup::run(
            &lua,
            &watchdog,
            crate::registry::TUI_SETUP_FN,
            "mineral.tui",
        )
        .map_err(|source| Error::Lua {
            operation: "执行 tui.lua 的 setup",
            source,
        })?;
        Ok((value, watchdog))
    })();
    let (value, watchdog) = evaluated.inspect_err(|source| {
        mineral_log::error!(
            target: "script",
            entry = name,
            error = mineral_log::chain(source),
            "TUI configuration or setup failed"
        );
    })?;
    let commands = host.take_commands();
    mineral_log::debug!(
        target: "script",
        entry = name,
        command_count = commands.len(),
        "TUI configuration and setup loaded"
    );
    Ok(TuiLoad {
        value,
        runtime: TuiRuntime {
            lua,
            host,
            watchdog,
        },
        commands,
    })
}

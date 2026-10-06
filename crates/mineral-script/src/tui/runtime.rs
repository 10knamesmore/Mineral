//! 在 TUI 进程中保留 Lua VM 与复制模板闭包，不创建脚本线程。

use std::sync::Arc;

use mlua::Lua;
use parking_lot::Mutex;

use super::TuiCommand;
use crate::{CopyTemplateCtx, Error, Result, SourceWebUrls, WatchdogConfig};

/// Lua API 的命令暂存区；每次成功求值或渲染后交还给 TUI。
#[derive(Clone, Debug, Default)]
pub(crate) struct TuiHost {
    /// 当前操作产生、尚未提交的命令，按调用顺序保存。
    commands: Arc<Mutex<Vec<TuiCommand>>>,
}

impl TuiHost {
    /// 保存已经通过 Lua 类型解析的本地命令。
    pub(crate) fn push(&self, command: TuiCommand) {
        self.commands.lock().push(command);
    }

    /// 取走或丢弃本次操作的全部命令，下一次操作从空缓冲区开始。
    pub(super) fn take_commands(&self) -> Vec<TuiCommand> {
        std::mem::take(&mut *self.commands.lock())
    }
}

/// TUI 本地脚本状态；复制回调的闭包随 VM 保留，重载时整体替换。
///
/// 由 TUI 输入循环同步调用；没有 daemon sender、后台线程或共享配置。
#[derive(Debug)]
pub struct TuiRuntime {
    /// 持有复制模板回调和其捕获状态的独立 Lua VM。
    pub(super) lua: Lua,

    /// 本次回调的本地效果暂存区。
    pub(super) host: TuiHost,

    /// 后续复制回调使用的看门狗参数，配置变化时由 TUI 重设。
    pub(super) watchdog: WatchdogConfig,
}

impl TuiRuntime {
    /// 更新后续回调的预算，保留当前 VM 和闭包状态。
    pub fn set_watchdog(&mut self, watchdog: WatchdogConfig) {
        self.watchdog = watchdog;
    }

    /// 同步渲染本地模板，成功时返回文本与有序效果；失败丢弃全部效果。
    ///
    /// # Params:
    ///   - `index`: tui.lua 的 copy.templates 零基下标
    ///   - `ctx`: TUI 已加载的音乐实体
    ///
    /// # Return:
    ///   剪贴板文本与本地命令。调用方仍须校验命令中的配置覆盖，再统一提交。
    ///   Lua 闭包状态在调用之间保留；回调失败不回滚其内部变量。
    pub fn render_copy_template(
        &self,
        index: usize,
        ctx: CopyTemplateCtx,
    ) -> Result<(String, Vec<TuiCommand>)> {
        let rendered = crate::copy::render(&self.lua, &self.watchdog, index, ctx);
        let commands = self.host.take_commands();
        let text = rendered?;
        mineral_log::debug!(
            target: "script",
            index,
            command_count = commands.len(),
            "local copy template rendered"
        );
        Ok((text, commands))
    }

    /// 保存各来源网页模板，之后的本地实体投影据此生成 url 字段。
    ///
    /// # Params:
    ///   - `urls`: TUI 已知的来源模板；缺席的实体网页链接为 nil
    ///
    /// # Return:
    ///   写入结果；不执行复制回调，也不产生界面效果。
    pub fn seed_web_url_templates(
        &self,
        urls: impl IntoIterator<Item = SourceWebUrls>,
    ) -> Result<()> {
        crate::seed_web_url_templates(&self.lua, urls).map_err(|source| Error::Lua {
            operation: "保存 TUI 网页链接模板",
            source,
        })
    }
}

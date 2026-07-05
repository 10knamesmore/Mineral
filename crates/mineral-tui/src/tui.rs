//! Terminal RAII guard。
//!
//! [`Tui::enter`] 切到 alternate screen + raw mode,[`Tui::exit`] / `Drop`
//! 必然恢复终端,即使发生 panic(我们在 enter 时安装了一个 chained panic hook)。

use std::fmt;
use std::io::{self, Stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::Command;
use crossterm::event::{
    DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    is_raw_mode_enabled,
};
use ratatui::Frame;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Position;

/// 终端 backend 的 RAII 持有者。
pub struct Tui {
    /// ratatui 的终端 backend(crossterm),Drop 时自动还原。
    terminal: Terminal<CrosstermBackend<Stdout>>,

    /// 进 alternate screen **前**捕获的原屏幕光标位置(通常是拉起 mineral 的 shell
    /// 提示符处),供整屏 expand/collapse 以其为缩放锚点。无 TTY / DSR 查询失败时为 `None`。
    launch_cursor: Option<Position>,

    /// 是否已成功 push 终端标题栈。panic hook 与正常恢复路径共用此 Arc,
    /// 保证只有真正 push 过才 pop。
    title_pushed: Arc<AtomicBool>,
}

impl Tui {
    /// 创建 backend(暂未进入 raw mode)。
    pub fn new() -> color_eyre::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            launch_cursor: None,
            title_pushed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// 进入 raw mode + alternate screen,并安装 panic hook 兜底恢复终端。
    pub fn enter(&mut self) -> color_eyre::Result<()> {
        enable_raw_mode()?;
        // 必须在切 alternate screen 前查:切屏后原屏幕(shell 提示符所在)的光标位置即不可得。
        // raw mode 已开,可读 DSR 响应;headless / 管道下查询失败则留 `None`,绝不阻断启动。
        self.launch_cursor = crossterm::cursor::position()
            .ok()
            .map(|(x, y)| Position { x, y });
        // focus 事件(mode 1004):FocusGained/FocusLost 驱动顶栏失焦变灰。
        // 不支持的终端忽略该序列、永不发事件,UI 恒按聚焦渲染。
        execute!(
            io::stdout(),
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableFocusChange
        )?;
        // kitty keyboard protocol:让 Shift+arrow / Ctrl+组合键 都带显式 modifier 上来。
        // 不开的话 kitty 默认把 Shift+Left 当裸 Left 报,丢了 SHIFT modifier
        // → 大跨度 seek 不生效。不支持协议的终端(macOS Terminal / iTerm2 旧版)
        // 会忽略该 escape sequence,无副作用,所以静默忽略错误。
        let _ = execute!(
            io::stdout(),
            PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
        );
        self.terminal.hide_cursor()?;
        self.terminal.clear()?;

        let prev = std::panic::take_hook();
        let title_pushed_for_hook = Arc::clone(&self.title_pushed);
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal(&title_pushed_for_hook);
            prev(info);
        }));
        Ok(())
    }

    /// push 终端标题栈(`CSI 22;2 t`),供退出时 pop 还原。
    ///
    /// 调用点必须在 [`Self::enter`] 之后、任何 `SetTitle` 之前。
    pub(crate) fn push_title_stack(&mut self) -> io::Result<()> {
        execute!(io::stdout(), PushWindowTitle)?;
        self.title_pushed.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 退出 alternate screen + raw mode,并 pop 标题栈(若之前 push 过)。多次调用幂等。
    pub fn exit(&mut self) -> color_eyre::Result<()> {
        restore_terminal(&self.title_pushed)?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// 进 alternate screen 前捕获的原屏幕光标位置;见字段文档。未捕获到为 `None`。
    pub fn launch_cursor(&self) -> Option<Position> {
        self.launch_cursor
    }

    /// 渲染一帧。
    pub fn draw<F>(&mut self, f: F) -> color_eyre::Result<()>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.terminal.draw(f)?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = restore_terminal(&self.title_pushed);
    }
}

/// 真正的终端恢复实现:仅当 raw mode 处于开启时才调用 disable,避免在
/// 未初始化的情况下报错。若标题栈已 push,先 pop 再还原其它状态。
fn restore_terminal(title_pushed: &Arc<AtomicBool>) -> io::Result<()> {
    if is_raw_mode_enabled().unwrap_or(false) {
        // 先 pop kitty keyboard protocol(没 push 成功的终端 ignore 即可),
        // 再 pop 标题栈,最后 LeaveAlternateScreen / 关 mouse capture / disable raw,
        // 顺序对称于 enter。
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        if title_pushed.load(Ordering::SeqCst) {
            let _ = execute!(io::stdout(), PopWindowTitle);
            title_pushed.store(false, Ordering::SeqCst);
        }
        disable_raw_mode()?;
        execute!(
            io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture,
            DisableFocusChange
        )?;
    }
    Ok(())
}

/// 标题栈 push 命令:`CSI 22;2 t`。
struct PushWindowTitle;

impl Command for PushWindowTitle {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b[22;2t")
    }
}

/// 标题栈 pop 命令:`CSI 23;2 t`。
struct PopWindowTitle;

impl Command for PopWindowTitle {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b[23;2t")
    }
}

#[cfg(test)]
mod tests {
    use super::{PopWindowTitle, PushWindowTitle};

    /// push 命令写入正确的 CSI 序列。
    #[test]
    fn push_window_title_emits_correct_sequence() -> color_eyre::Result<()> {
        let mut buf = Vec::<u8>::new();
        crossterm::execute!(buf, PushWindowTitle)?;
        assert_eq!(String::from_utf8(buf)?, "\x1b[22;2t");
        Ok(())
    }

    /// pop 命令写入正确的 CSI 序列。
    #[test]
    fn pop_window_title_emits_correct_sequence() -> color_eyre::Result<()> {
        let mut buf = Vec::<u8>::new();
        crossterm::execute!(buf, PopWindowTitle)?;
        assert_eq!(String::from_utf8(buf)?, "\x1b[23;2t");
        Ok(())
    }
}

//! Terminal RAII guard。
//!
//! [`Tui::enter`] 切到 alternate screen + raw mode,[`Tui::exit`] / `Drop`
//! 必然恢复终端,即使发生 panic(我们在 enter 时安装了一个 chained panic hook)。

use std::io::{self, Stdout};

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
}

impl Tui {
    /// 创建 backend(暂未进入 raw mode)。
    pub fn new() -> color_eyre::Result<Self> {
        let backend = CrosstermBackend::new(io::stdout());
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            launch_cursor: None,
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
        std::panic::set_hook(Box::new(move |info| {
            let _ = restore_terminal();
            prev(info);
        }));
        Ok(())
    }

    /// 退出 alternate screen + raw mode。多次调用幂等。
    pub fn exit(&mut self) -> color_eyre::Result<()> {
        restore_terminal()?;
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
        let _ = restore_terminal();
    }
}

/// 真正的终端恢复实现:仅当 raw mode 处于开启时才调用 disable,避免在
/// 未初始化的情况下报错。
fn restore_terminal() -> io::Result<()> {
    if is_raw_mode_enabled().unwrap_or(false) {
        // 先 pop kitty keyboard protocol(没 push 成功的终端 ignore 即可),
        // 再 LeaveAlternateScreen / 关 mouse capture / disable raw,顺序对称于 enter。
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
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

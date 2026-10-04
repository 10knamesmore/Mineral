//! Terminal RAII guard。
//!
//! [`Tui::enter`] 切到 alternate screen + raw mode,[`Tui::exit`] / `Drop`
//! 必然恢复终端,即使发生 panic(我们在 enter 时安装了一个 chained panic hook)。

use std::fmt;
use std::io::{self, BufWriter, Stdout, Write as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crossterm::Command;
use crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    is_raw_mode_enabled,
};
use ratatui::Terminal;
use ratatui::backend::{Backend as _, CrosstermBackend};
use ratatui::layout::Position;

use crate::image::graphics::GraphicsProtocol;

/// 终端 backend 的 RAII 持有者。
pub struct Tui {
    /// ratatui 的终端 backend(crossterm),Drop 时自动还原。
    terminal: Terminal<CrosstermBackend<BufWriter<Stdout>>>,

    /// 进 alternate screen **前**捕获的原屏幕光标位置(通常是拉起 mineral 的 shell
    /// 提示符处),供整屏 expand/collapse 以其为缩放锚点。无 TTY / DSR 查询失败时为 `None`。
    launch_cursor: Option<Position>,

    /// 是否已成功 push 终端标题栈。panic hook 与正常恢复路径共用此 Arc,
    /// 保证只有真正 push 过才 pop。
    title_pushed: Arc<AtomicBool>,

    /// 与终端同生命周期的保留画布和区域输出参照。
    compositor: crate::render::memo::Compositor,

    /// 已连续省略的帧数，恢复提交时记录，避免逐帧日志。
    skipped_frames: u64,
}

impl Tui {
    /// 创建 backend(暂未进入 raw mode)。
    pub fn new() -> io::Result<Self> {
        let backend = CrosstermBackend::new(BufWriter::with_capacity(64 * 1024, io::stdout()));
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            launch_cursor: None,
            title_pushed: Arc::new(AtomicBool::new(false)),
            compositor: crate::render::memo::Compositor::default(),
            skipped_frames: 0,
        })
    }

    /// 进入 raw mode + alternate screen,并安装 panic hook 兜底恢复终端。
    pub fn enter(&mut self) -> io::Result<()> {
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
            EnableFocusChange,
            EnableBracketedPaste
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

    /// push 终端标题栈(`CSI 22;0 t`),供退出时 pop 还原。**幂等**:已 push 过即空转。
    ///
    /// 调用点必须在 [`Self::enter`] 之后、任何 `SetTitle` 之前。主循环每帧在标题启用时
    /// 兜一次(见 `App::run`),把热重载从禁用→启用也纳入——否则运行时才开启会写标题却
    /// 从未 push,退出无从 pop、残留 stale 标题。
    pub(crate) fn push_title_stack(&mut self) -> io::Result<()> {
        if self.title_pushed.load(Ordering::SeqCst) {
            return Ok(());
        }
        execute!(io::stdout(), PushWindowTitle)?;
        self.title_pushed.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// 退出 alternate screen + raw mode,并 pop 标题栈(若之前 push 过)。多次调用幂等。
    pub fn exit(&mut self) -> io::Result<()> {
        self.discard_pending_output();
        restore_terminal(&self.title_pushed)?;
        self.terminal.show_cursor()?;
        Ok(())
    }

    /// 进 alternate screen 前捕获的原屏幕光标位置;见字段文档。未捕获到为 `None`。
    pub fn launch_cursor(&self) -> Option<Position> {
        self.launch_cursor
    }

    /// 同步终端尺寸，供独立于绘制的布局准备使用。
    pub(crate) fn area(&mut self) -> io::Result<ratatui::layout::Rect> {
        self.terminal.autoresize()?;
        Ok(self.terminal.get_frame().area())
    }

    /// 输入或合成变化才提交；图片指令独立检查，始终先于引用它的 cell 输出。
    pub(crate) fn present(
        &mut self,
        plan: crate::render::memo::FramePlan<'_>,
        graphics: &str,
        protocol: GraphicsProtocol,
    ) -> io::Result<()> {
        let area = self.terminal.get_frame().area();
        if !self.compositor.needs_frame(&plan, area) && graphics.is_empty() {
            if self.skipped_frames == 0 {
                mineral_log::debug!(target: "tui::render", "visible components unchanged; suspend frame submissions");
            }
            self.skipped_frames = self.skipped_frames.saturating_add(1);
            return Ok(());
        }
        let whole_frame_graphics =
            matches!(protocol, GraphicsProtocol::Sixel | GraphicsProtocol::Iterm2);
        let update =
            self.compositor
                .render(plan, &mut self.terminal.get_frame(), whole_frame_graphics);
        if !graphics.is_empty() {
            let mut output = io::stdout().lock();
            output.write_all(graphics.as_bytes())?;
            output.flush()?;
        }
        if !update.is_empty() || !graphics.is_empty() {
            self.terminal
                .backend_mut()
                .draw(self.compositor.updates(&update))?;
            self.terminal.hide_cursor()?;
            ratatui::backend::Backend::flush(self.terminal.backend_mut())?;
        }
        if self.skipped_frames > 0 {
            mineral_log::debug!(target: "tui::render", skipped_frames = self.skipped_frames,
                painted_components = update.painted_components, changed_cells = update.len(),
                "frame composition resumed");
            self.skipped_frames = 0;
        }
        Ok(())
    }

    /// 丢弃未完成帧，避免 BufWriter 在终端恢复后析构时补写到 shell。
    fn discard_pending_output(&mut self) {
        // panic hook 可能已恢复终端；不能在此 flush。后续仅有光标恢复命令，直接写出。
        let pending = std::mem::replace(
            self.terminal.backend_mut().writer_mut(),
            BufWriter::with_capacity(0, io::stdout()),
        );
        let _ = pending.into_parts();
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        self.discard_pending_output();
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
            DisableFocusChange,
            DisableBracketedPaste
        )?;
    }
    Ok(())
}

/// 标题栈 push 命令:`CSI 22;0 t`(Ps=0 = 同时存图标名 + 窗口标题)。
/// 与 `SetTitle`(OSC 0,一发同改二者)对称——用 Ps=2 只存窗口标题,退出 pop
/// 时图标名不还原、残留 stale 标题。
struct PushWindowTitle;

impl Command for PushWindowTitle {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b[22;0t")
    }
}

/// 标题栈 pop 命令:`CSI 23;0 t`(Ps=0 = 同时还原图标名 + 窗口标题)。
struct PopWindowTitle;

impl Command for PopWindowTitle {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        f.write_str("\x1b[23;0t")
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
        assert_eq!(String::from_utf8(buf)?, "\x1b[22;0t");
        Ok(())
    }

    /// pop 命令写入正确的 CSI 序列。
    #[test]
    fn pop_window_title_emits_correct_sequence() -> color_eyre::Result<()> {
        let mut buf = Vec::<u8>::new();
        crossterm::execute!(buf, PopWindowTitle)?;
        assert_eq!(String::from_utf8(buf)?, "\x1b[23;0t");
        Ok(())
    }
}

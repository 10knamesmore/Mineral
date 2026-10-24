use std::io::{self, Write};

use ratatui::buffer::Cell;
use ratatui::crossterm::cursor::MoveTo;
use ratatui::crossterm::queue;
use ratatui::crossterm::style::{
    Attribute, Color as TerminalColor, Colors, Print, SetAttribute, SetBackgroundColor, SetColors,
    SetForegroundColor, SetUnderlineColor,
};
use ratatui::layout::Position;
use ratatui::style::{Color, Modifier};

/// 沿用 ratatui 0.29 的 cell 输出顺序与帧末重置，只省略未变化的前景或背景通道。
pub(super) fn write_cells<'a, W, I>(writer: &mut W, cells: I) -> io::Result<()>
where
    W: Write,
    I: Iterator<Item = (u16, u16, &'a Cell)>,
{
    let mut foreground = Color::Reset;
    let mut background = Color::Reset;
    let mut underline_color = Color::Reset;
    let mut modifiers = Modifier::empty();
    let mut last_position: Option<Position> = None;

    for (x, y, cell) in cells {
        if !matches!(last_position, Some(position) if x == position.x + 1 && y == position.y) {
            queue!(writer, MoveTo(x, y))?;
        }
        last_position = Some(Position { x, y });

        if cell.modifier != modifiers {
            write_modifier_diff(writer, modifiers, cell.modifier)?;
            modifiers = cell.modifier;
        }
        if cell.fg != foreground || cell.bg != background {
            queue!(
                writer,
                SetColors(Colors {
                    foreground: (cell.fg != foreground).then(|| cell.fg.into()),
                    background: (cell.bg != background).then(|| cell.bg.into()),
                })
            )?;
            foreground = cell.fg;
            background = cell.bg;
        }
        // Kitty Unicode placeholder 的 placement id 使用下划线颜色，必须独立于前景和背景。
        if cell.underline_color != underline_color {
            queue!(
                writer,
                SetUnderlineColor(TerminalColor::from(cell.underline_color))
            )?;
            underline_color = cell.underline_color;
        }

        queue!(writer, Print(cell.symbol()))?;
    }

    queue!(
        writer,
        SetForegroundColor(TerminalColor::Reset),
        SetBackgroundColor(TerminalColor::Reset),
        SetUnderlineColor(TerminalColor::Reset),
        SetAttribute(Attribute::Reset),
    )
}

// 保持 ratatui 0.29 ModifierDiff 的命令顺序；NormalIntensity 同时清除 BOLD 和 DIM。
fn write_modifier_diff<W: Write>(
    writer: &mut W,
    previous: Modifier,
    next: Modifier,
) -> io::Result<()> {
    let removed = previous - next;
    if removed.contains(Modifier::REVERSED) {
        queue!(writer, SetAttribute(Attribute::NoReverse))?;
    }
    if removed.contains(Modifier::BOLD) {
        queue!(writer, SetAttribute(Attribute::NormalIntensity))?;
        if next.contains(Modifier::DIM) {
            queue!(writer, SetAttribute(Attribute::Dim))?;
        }
    }
    if removed.contains(Modifier::ITALIC) {
        queue!(writer, SetAttribute(Attribute::NoItalic))?;
    }
    if removed.contains(Modifier::UNDERLINED) {
        queue!(writer, SetAttribute(Attribute::NoUnderline))?;
    }
    if removed.contains(Modifier::DIM) {
        queue!(writer, SetAttribute(Attribute::NormalIntensity))?;
    }
    if removed.contains(Modifier::CROSSED_OUT) {
        queue!(writer, SetAttribute(Attribute::NotCrossedOut))?;
    }
    if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
        queue!(writer, SetAttribute(Attribute::NoBlink))?;
    }

    let added = next - previous;
    if added.contains(Modifier::REVERSED) {
        queue!(writer, SetAttribute(Attribute::Reverse))?;
    }
    if added.contains(Modifier::BOLD) {
        queue!(writer, SetAttribute(Attribute::Bold))?;
    }
    if added.contains(Modifier::ITALIC) {
        queue!(writer, SetAttribute(Attribute::Italic))?;
    }
    if added.contains(Modifier::UNDERLINED) {
        queue!(writer, SetAttribute(Attribute::Underlined))?;
    }
    if added.contains(Modifier::DIM) {
        queue!(writer, SetAttribute(Attribute::Dim))?;
    }
    if added.contains(Modifier::CROSSED_OUT) {
        queue!(writer, SetAttribute(Attribute::CrossedOut))?;
    }
    if added.contains(Modifier::SLOW_BLINK) {
        queue!(writer, SetAttribute(Attribute::SlowBlink))?;
    }
    if added.contains(Modifier::RAPID_BLINK) {
        queue!(writer, SetAttribute(Attribute::RapidBlink))?;
    }

    Ok(())
}

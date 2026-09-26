//! 页面内容转场：按端点尺寸绘制、裁剪到当前区域，并以同一进度交接文字。

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Widget};

use crate::render::anim::ease_in_out;
use crate::render::blit;
use crate::render::color::lerp_color;
use crate::render::theme::Theme;

/// 把现有组件按固定区域画到离屏缓冲，返回后恢复主帧。
///
/// 调用方负责冻结滚动和选择纯 cell 图片阶段。背景沿用屏幕对应位置，绘制后把未改变的
/// 背景还原为透明，避免搬运文字时把氛围背景一起搬走。
pub(crate) fn capture(
    frame: &mut Frame<'_>,
    area: Rect,
    paint: impl FnOnce(&mut Frame<'_>),
) -> Buffer {
    let mut offscreen = Buffer::empty(area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let (Some(under), Some(cell)) =
                (frame.buffer_mut().cell((x, y)), offscreen.cell_mut((x, y)))
            {
                cell.set_bg(under.bg);
            }
        }
    }
    let screen = std::mem::replace(frame.buffer_mut(), offscreen);
    paint(frame);
    let mut painted = std::mem::replace(frame.buffer_mut(), screen);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let (Some(under), Some(cell)) =
                (frame.buffer_mut().cell((x, y)), painted.cell_mut((x, y)))
                && cell.bg == under.bg
            {
                cell.set_bg(Color::Reset);
            }
        }
    }
    painted
}

/// 绘制带一格圆角边框的面板：外框持续存在，标题和正文按端点排版淡化交接。
pub(crate) fn panel(
    frame: &mut Frame<'_>,
    from: Option<&Buffer>,
    to: Option<&Buffer>,
    area: Rect,
    raw_progress: u16,
    theme: &Theme,
) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    let buf = frame.buffer_mut();
    clear_symbols(buf, area);
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border_color(from, to, raw_progress, theme)));
    let inner = block.inner(area);
    block.render(area, buf);
    let Some((source, opacity)) = visible_content(from, to, raw_progress) else {
        return;
    };
    let source_inner = Block::new().borders(Borders::ALL).inner(source.area);
    paint_window(buf, source, source_inner, inner, opacity, theme, None);
    for (source_y, target_y) in [
        (source.area.top(), area.top()),
        (
            source.area.bottom().saturating_sub(1),
            area.bottom().saturating_sub(1),
        ),
    ] {
        paint_window(
            buf,
            source,
            Rect::new(source_inner.x, source_y, source_inner.width, 1),
            Rect::new(inner.x, target_y, inner.width, 1),
            opacity,
            theme,
            Some("─"),
        );
    }
    // minimap 位于侧边框：标记跟随正文淡化，默认竖线由当前外框保留。
    for (source_x, target_x) in [
        (source.area.left(), area.left()),
        (
            source.area.right().saturating_sub(1),
            area.right().saturating_sub(1),
        ),
    ] {
        paint_window(
            buf,
            source,
            Rect::new(source_x, source_inner.y, 1, source_inner.height),
            Rect::new(target_x, inner.y, 1, inner.height),
            opacity,
            theme,
            Some("│"),
        );
    }
}

/// 绘制无边框内容，供顶栏和独立文字区域淡化使用。
pub(crate) fn content(
    frame: &mut Frame<'_>,
    from: Option<&Buffer>,
    to: Option<&Buffer>,
    area: Rect,
    raw_progress: u16,
    theme: &Theme,
) {
    if let Some((source, opacity)) = visible_content(from, to, raw_progress) {
        paint_window(
            frame.buffer_mut(),
            source,
            source.area,
            area,
            opacity,
            theme,
            None,
        );
    }
}

/// 出发文字在前半程淡出，目标文字从 42% 起接入；取较亮的一端，避免叠印不同字形。
/// 取值仅依赖位置，中途反向不会重新开始文字转场。
fn visible_content<'a>(
    from: Option<&'a Buffer>,
    to: Option<&'a Buffer>,
    progress: u16,
) -> Option<(&'a Buffer, u16)> {
    let progress = u32::from(progress.min(1000));
    let outgoing = 1000 - smoothstep((progress * 1000 / 500).min(1000));
    let incoming = smoothstep(progress.saturating_sub(420) * 1000 / 580);
    let selected = match (from, to) {
        (Some(from), Some(_)) if outgoing >= incoming => (from, outgoing),
        (Some(_), Some(to)) => (to, incoming),
        (Some(from), None) => (from, outgoing),
        (None, Some(to)) => (to, incoming),
        (None, None) => return None,
    };
    (selected.1 > 0).then_some(selected)
}

/// 千分比 smoothstep，两端速度为零，供文字明暗交接使用。
fn smoothstep(progress: u32) -> u16 {
    let p = u64::from(progress.min(1000));
    u16::try_from(p * p * (3000 - 2 * p) / 1_000_000).unwrap_or(1000)
}

/// 外框颜色沿几何进度变化，内容淡出时不带走边界。
fn border_color(from: Option<&Buffer>, to: Option<&Buffer>, raw: u16, theme: &Theme) -> Color {
    let color = |buffer: &Buffer| {
        buffer
            .cell((buffer.area.x, buffer.area.y))
            .map_or(theme.surface1, |cell| cell.fg)
    };
    match (from, to) {
        (Some(from), Some(to)) => {
            lerp_color(color(from), color(to), u64::from(ease_in_out(raw)), 1000)
        }
        (Some(buffer), None) | (None, Some(buffer)) => color(buffer),
        (None, None) => theme.surface1,
    }
}

/// 清除当前区域的字符与修饰，保留每格实际背景。
pub(crate) fn clear_symbols(buf: &mut Buffer, area: Rect) {
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(cell) = buf.cell_mut((x, y)) {
                let background = cell.bg;
                cell.reset();
                cell.set_bg(background);
            }
        }
    }
}

/// 搬运固定排版的窗口；默认边线留给当前外框，标题和边框标记正常淡化。
fn paint_window(
    dst: &mut Buffer,
    source: &Buffer,
    source_area: Rect,
    target: Rect,
    opacity: u16,
    theme: &Theme,
    border_symbol: Option<&str>,
) {
    let window = Rect::new(
        source_area.x,
        source_area.y,
        source_area.width.min(target.width),
        source_area.height.min(target.height),
    );
    let target = Rect::new(target.x, target.y, window.width, window.height);
    let mut layer = Buffer::empty(target);
    blit::copy_window(&mut layer, source, window, target.x, target.y);
    for y in target.top()..target.bottom() {
        for x in target.left()..target.right() {
            let (Some(source), Some(destination)) = (layer.cell((x, y)), dst.cell_mut((x, y)))
            else {
                continue;
            };
            if border_symbol == Some(source.symbol()) {
                continue;
            }
            let background = match destination.bg {
                Color::Reset => theme.base,
                background => background,
            };
            let foreground = match source.fg {
                Color::Reset => theme.text,
                foreground => foreground,
            };
            let mut cell = source.clone();
            // Kitty 占位字符的前景色编码 image id，淡化会指向另一张图；图片随内容移动和切换。
            if !cell.symbol().starts_with('\u{10EEEE}') {
                cell.set_fg(lerp_color(background, foreground, u64::from(opacity), 1000));
            }
            cell.set_bg(match source.bg {
                Color::Reset => destination.bg,
                color => lerp_color(background, color, u64::from(opacity), 1000),
            });
            *destination = cell;
        }
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::widgets::{Block, BorderType, Borders, Widget};

    use super::{capture, panel};

    /// 形变搬运和裁切缩略图时保留图片与列标识，文字淡化不能改写 image id。
    #[test]
    fn thumbnail_identity_survives_motion_clipping_and_text_fade() -> color_eyre::Result<()> {
        use std::sync::Arc;

        use mineral_model::MediaUrl;

        use crate::image::{ImageEngine, ImageRenderPhase};

        let theme = crate::test_support::default_theme()?;
        let images = ImageEngine::disabled_kitty(Arc::new(mineral_config::Config::defaults()?));
        let url = MediaUrl::remote("https://example.com/morph-cover.png")?;
        images.insert_test_thumbnail(&url)?;
        let area = Rect::new(0, 0, 4, 1);
        let mut source = Buffer::empty(area);
        source.set_string(2, 0, "AB", Style::new().fg(theme.text));
        images.render_thumbnail(
            Some(&url),
            Rect::new(0, 0, 2, 1),
            &mut source,
            ImageRenderPhase::Offscreen,
        );
        let mut destination = Buffer::empty(Rect::new(0, 0, 12, 4));
        super::paint_window(
            &mut destination,
            &source,
            area,
            Rect::new(4, 2, 4, 1),
            500,
            &theme,
            None,
        );
        for x in 0..2 {
            let original = source.cell((x, 0)).ok_or_else(|| eyre!("缺少原始封面格"))?;
            let moved = destination
                .cell((x + 4, 2))
                .ok_or_else(|| eyre!("缺少移动封面格"))?;
            assert!(moved.symbol().starts_with('\u{10EEEE}'));
            assert_eq!(
                (moved.symbol(), moved.fg, moved.underline_color),
                (original.symbol(), original.fg, original.underline_color)
            );
        }
        super::paint_window(
            &mut destination,
            &source,
            Rect::new(1, 0, 3, 1),
            Rect::new(1, 3, 1, 1),
            500,
            &theme,
            None,
        );
        assert_eq!(
            destination.cell((1, 3)),
            source.cell((1, 0)),
            "裁切后保留缩略图第二列，不重新拉伸"
        );
        assert_eq!(
            destination.cell((2, 3)).map(ratatui::buffer::Cell::symbol),
            Some(" "),
            "不能溢出裁切窗口"
        );
        Ok(())
    }

    /// 文字先在固定宽度排版，再裁剪到当前内区；边框持续存在，CJK 不越过边界。
    #[test]
    fn fixed_content_is_clipped_without_reflow() -> color_eyre::Result<()> {
        let theme = crate::test_support::default_theme()?;
        let source_area = Rect::new(0, 0, 16, 5);
        let mut source = Buffer::empty(source_area);
        Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme.surface1))
            .render(source_area, &mut source);
        source.set_string(1, 1, "甲乙丙丁", Style::new().fg(theme.text));
        source.set_string(1, 2, "second row", Style::new().fg(theme.text));
        source.set_string(15, 2, "⣿", Style::new().fg(theme.accent));
        let mut terminal = Terminal::new(TestBackend::new(20, 8))?;
        terminal.draw(|frame| {
            panel(
                frame,
                Some(&source),
                None,
                Rect::new(3, 1, 7, 5),
                100,
                &theme,
            );
        })?;
        let buf = terminal.backend().buffer();
        let cell = |x, y| buf.cell((x, y)).ok_or_else(|| eyre!("missing cell"));
        assert_eq!(cell(4, 2)?.symbol(), "甲");
        assert_eq!(cell(6, 2)?.symbol(), "乙");
        assert_eq!(cell(8, 2)?.symbol(), " ", "被切开的宽字符留空");
        assert_eq!(cell(9, 2)?.symbol(), "│", "右边界保持完整");
        assert_eq!(cell(4, 3)?.symbol(), "s", "下一行仍来自原排版");
        Ok(())
    }

    /// 捕获结束恢复主帧，未改变的背景保持透明，后续淡化针对落点的实际底色。
    #[test]
    fn capture_restores_frame_and_keeps_backdrop_transparent() -> color_eyre::Result<()> {
        let area = Rect::new(0, 0, 8, 3);
        let background = Color::Rgb(12, 24, 36);
        let mut terminal = Terminal::new(TestBackend::new(8, 3))?;
        terminal.draw(|frame| {
            frame.render_widget(Block::new().style(Style::new().bg(background)), area);
            let layer = capture(frame, area, |frame| {
                frame
                    .buffer_mut()
                    .set_string(1, 1, "old", Style::new().fg(Color::Red));
            });
            assert_eq!(layer.cell((1, 1)).map(|c| c.bg), Some(Color::Reset));
            assert_eq!(
                frame
                    .buffer_mut()
                    .cell((1, 1))
                    .map(ratatui::buffer::Cell::symbol),
                Some(" ")
            );
            assert_eq!(
                frame.buffer_mut().cell((1, 1)).map(|c| c.bg),
                Some(background)
            );
        })?;
        Ok(())
    }
}

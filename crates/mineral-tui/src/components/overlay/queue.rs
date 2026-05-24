//! 浮动 queue 面板。

use mineral_model::{Song, SongId};
use ratatui::Frame;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Row, Table, TableState};

use crate::components::overlay::centered_rect;
use crate::playback::format_ms;
use crate::theme::Theme;

/// 缩放进度满值(千分比)。等于 [`crate::anim::Transition::ease_out`] 的满值,
/// 到此即「完全展开」,渲染完整表格;不足则只画缩放中的空壳。
const FULL_SCALE: u16 = 1000;

/// queue 浮层一次渲染所需的全部输入。
///
/// 单独打包是为了让 [`draw`] 参数收敛,字段都是渲染期借用/瞬时值,非持久配置。
pub struct QueueRender<'a> {
    /// 队列曲目(平铺顺序)。
    pub queue: &'a [Song],

    /// 光标选中行下标。
    pub sel: usize,

    /// 当前在播歌 id(该行行首标 `▶`);无在播为 `None`。
    pub current_id: Option<&'a SongId>,

    /// 浮层是否持有键盘焦点(决定边框色)。
    pub focused: bool,

    /// 缩放弹出进度(千分比 `0..=1000`,已缓动)。`< 1000` 时只画空壳。
    pub scale: u16,
}

/// 渲染 queue 浮层,以 `area`(主帧区域)为参考居中,按 `render.scale` 从中心缩放弹出。
///
/// # Params:
///   - `render`: 本次渲染的数据与展示态,见 [`QueueRender`]。
pub fn draw(frame: &mut Frame<'_>, area: Rect, render: &QueueRender<'_>, theme: &Theme) {
    let base = centered_rect(area, 60, 70, 40, 12, 96, 32);
    let panel = scale_rect(base, render.scale);
    // 缩放初期面板太小,画不出有意义的边框,直接跳过这一帧。
    if panel.width < 4 || panel.height < 3 {
        return;
    }

    frame.render_widget(Clear, panel);
    let block = panel_block(theme, render.focused, render.sel, render.queue.len());

    if render.scale >= FULL_SCALE {
        draw_table(frame, panel, block, render, theme);
    } else {
        // 动画途中只画空壳(边框 + 背景),不画表格 —— 避免窄尺寸下表格 reflow 抖动。
        frame.render_widget(block, panel);
    }
}

/// 构造浮层外框:圆角边框 + 标题 + 底部位置/导航提示。
fn panel_block<'a>(theme: &Theme, focused: bool, sel: usize, total: usize) -> Block<'a> {
    let border_color = if focused {
        theme.accent
    } else {
        theme.surface1
    };
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border_color))
        .style(Style::new().bg(theme.mantle))
        .title(Line::from(" queue ").style(Style::new().fg(theme.subtext)))
        .title_bottom(Line::from(position_label(sel, total)).style(Style::new().fg(theme.overlay)))
        .title_bottom(
            Line::from(" ↵ play  ·  Tab/q/esc close ")
                .right_aligned()
                .style(Style::new().fg(theme.overlay)),
        )
}

/// 完全展开态:在 `panel` 内渲染带表头的曲目表格。
fn draw_table(
    frame: &mut Frame<'_>,
    panel: Rect,
    block: Block<'_>,
    render: &QueueRender<'_>,
    theme: &Theme,
) {
    let header = Row::new(vec![
        Cell::from("#"),
        Cell::from("title"),
        Cell::from("artist"),
        Cell::from("len"),
    ])
    .style(Style::new().fg(theme.subtext).add_modifier(Modifier::BOLD));

    let rows: Vec<Row<'_>> = render
        .queue
        .iter()
        .enumerate()
        .map(|(i, s)| build_row(i, s, render.current_id, theme))
        .collect();

    let widths = [
        Constraint::Length(3),
        Constraint::Min(10),
        Constraint::Length(16),
        Constraint::Length(6),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(
            Style::new()
                .bg(theme.surface0)
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▌ ");

    let mut state = TableState::default();
    state.select(Some(render.sel));
    frame.render_stateful_widget(table, panel, &mut state);
}

/// 把一首歌组成 queue 表格的一行。
///
/// 当前在播行:行首 `▶` + 整行 `accent` 前景(与选中行的「背景块」区分);其余行
/// 序号用暗色,歌名用主文本色,艺术家用次要色,层级分明。选中行的高亮背景由
/// Table 的 `row_highlight_style` 叠加,与在播前景着色天然兼容(背景块视觉优先)。
fn build_row<'a>(idx: usize, s: &'a Song, current_id: Option<&SongId>, theme: &Theme) -> Row<'a> {
    let is_current = current_id.is_some_and(|cid| cid == &s.id);
    let (lead, title_fg, sub_fg) = if is_current {
        (
            Span::styled("▶", Style::new().fg(theme.accent)),
            theme.accent,
            theme.accent,
        )
    } else {
        (
            Span::styled(format!("{idx}"), Style::new().fg(theme.overlay)),
            theme.text,
            theme.subtext,
        )
    };

    let artist = s
        .artists
        .first()
        .map_or_else(|| "—".to_owned(), |a| a.name.clone());

    Row::new(vec![
        Cell::from(lead),
        Cell::from(Span::styled(s.name.clone(), Style::new().fg(title_fg))),
        Cell::from(Span::styled(artist, Style::new().fg(sub_fg))),
        Cell::from(Span::styled(
            format_ms(s.duration_ms),
            Style::new().fg(sub_fg),
        )),
    ])
}

/// 拼 ` n / total ` 的 footer 标签;空 queue 显示 `0 / 0`。
fn position_label(sel: usize, total: usize) -> String {
    if total == 0 {
        " 0 / 0 ".to_owned()
    } else {
        format!(" {} / {total} ", sel.saturating_add(1).min(total))
    }
}

/// 以 `base` 中心为锚,按 `scale`(千分比 `0..=1000`)缩放出实际面板矩形。
fn scale_rect(base: Rect, scale: u16) -> Rect {
    let w = scaled_dim(base.width, scale);
    let h = scaled_dim(base.height, scale);
    let cx = base.x + base.width / 2;
    let cy = base.y + base.height / 2;
    let x = cx.saturating_sub(w / 2);
    let y = cy.saturating_sub(h / 2);
    Rect::new(x, y, w, h)
}

/// 把一个维度按千分比缩放,纯整数定点(避免浮点强转)。
fn scaled_dim(dim: u16, scale: u16) -> u16 {
    let v = u32::from(dim) * u32::from(scale) / u32::from(FULL_SCALE);
    u16::try_from(v).unwrap_or(dim)
}

#[cfg(test)]
mod tests {
    use mineral_model::{SongId, SourceKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::QueueRender;
    use crate::test_support::endserenading;
    use crate::theme::Theme;

    /// 空 queue,完全展开。
    #[test]
    fn queue_empty_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(60, 20))?;
        let render = QueueRender {
            queue: &[],
            sel: 0,
            current_id: None,
            focused: true,
            scale: 1000,
        };
        t.draw(|f| super::draw(f, f.area(), &render, &Theme::default()))?;
        crate::test_support::assert_snap!("队列浮层:空队列", t.backend());
        Ok(())
    }

    /// EndSerenading 前 3 曲 + 当前在播标记(Palisade)+ 聚焦,完全展开。
    #[test]
    fn queue_with_items_focused_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(60, 20))?;
        let songs = endserenading(3);
        let current = SongId::new(SourceKind::NETEASE, "2");
        let render = QueueRender {
            queue: &songs,
            sel: 0,
            current_id: Some(&current),
            focused: true,
            scale: 1000,
        };
        t.draw(|f| super::draw(f, f.area(), &render, &Theme::default()))?;
        crate::test_support::assert_snap!(
            "队列浮层:EndSerenading 前 3 曲,Palisade 当前在播(▶)+ 聚焦",
            t.backend()
        );
        Ok(())
    }

    /// 弹出动画半程(scale=500):只画缩放后的空壳边框,无表格内容。
    #[test]
    fn queue_mid_animation_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(60, 20))?;
        let songs = endserenading(3);
        let render = QueueRender {
            queue: &songs,
            sel: 0,
            current_id: None,
            focused: true,
            scale: 500,
        };
        t.draw(|f| super::draw(f, f.area(), &render, &Theme::default()))?;
        crate::test_support::assert_snap!("队列浮层:弹出动画半程(scale=500)空壳", t.backend());
        Ok(())
    }
}

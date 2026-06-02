//! 平铺界面的整屏几何变换动画:一个整 cell 边框在区域内按千分比进度朝锚点缩放,
//! 框内保留已画内容、框外清成背景色。启动扩大与退出收缩共用此实现,缩放方向只取决于
//! 进度走向,渲染本身方向无关。

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear};

use crate::render::theme::Theme;

/// 收缩进度满值(千分比),对齐 [`crate::render::anim::Transition::eased`] 的满值。
const FULL_SCALE: u32 = 1000;

/// 整屏缩放裁剪:一个整 cell 边框在 `area` 内按 `scale`(千分比)朝 `anchor` 缩放,框内保留
/// 已画的界面内容(静止),框外清成背景色。`scale` 由 0 涨到满即启动扩大、由满收到 0 即退出
/// 收缩 —— 方向只取决于 `scale` 的走向,渲染本身方向无关。
///
/// `anchor` 为进 alternate screen 前捕获的光标位置:expand 从此点铺开、collapse 收回此点。
/// `None`(无 TTY)时退化回屏幕居中,即历史行为。
pub fn clip_scaled(
    frame: &mut Frame<'_>,
    area: Rect,
    scale: u16,
    anchor: Option<Position>,
    theme: &Theme,
) {
    let scale = u32::from(scale);
    let w = u16::try_from(u32::from(area.width) * scale / FULL_SCALE).unwrap_or(area.width);
    let h = u16::try_from(u32::from(area.height) * scale / FULL_SCALE).unwrap_or(area.height);

    let area_bottom = area.y.saturating_add(area.height);
    let area_right = area.x.saturating_add(area.width);
    // 框左上角定位:有锚点则朝锚点插值,无锚点(无 TTY)退化回居中。
    // 居中分支逐字节沿用历史公式(`(area - 框) / 2`),与定点取整顺序一致 → 历史快照不变;
    // 锚点分支才走新插值,把改动收窄到「确有光标位置」时。
    let (x, y) = match anchor {
        Some(p) => {
            // clamp 进 area(防 enter 后终端缩小致 row/col 越界);`.max(area.{x,y})` 保证
            // clamp 的 min ≤ max(空 area 时也不 panic)。
            let ax = p.x.clamp(area.x, area_right.saturating_sub(1).max(area.x));
            let ay = p.y.clamp(area.y, area_bottom.saturating_sub(1).max(area.y));
            // scale=满 → 贴 area 原点(全屏);scale=0 → 收到锚点(成一点)。锚点在框内
            // 相对比例恒定 → 真正"从该点放 / 朝该点收"。全程非负定点,不碰 as 强转。
            let dx = u32::from(ax - area.x) * (FULL_SCALE - scale) / FULL_SCALE;
            let dy = u32::from(ay - area.y) * (FULL_SCALE - scale) / FULL_SCALE;
            (
                area.x.saturating_add(u16::try_from(dx).unwrap_or(0)),
                area.y.saturating_add(u16::try_from(dy).unwrap_or(0)),
            )
        }
        None => (
            area.x + area.width.saturating_sub(w) / 2,
            area.y + area.height.saturating_sub(h) / 2,
        ),
    };

    // 框外四条 bar 清成背景色,把界面"吞掉"。
    let below = y.saturating_add(h);
    let right = x.saturating_add(w);
    fill_bg(
        frame,
        Rect::new(area.x, area.y, area.width, y.saturating_sub(area.y)),
        theme.base,
    );
    fill_bg(
        frame,
        Rect::new(area.x, below, area.width, area_bottom.saturating_sub(below)),
        theme.base,
    );
    fill_bg(
        frame,
        Rect::new(area.x, y, x.saturating_sub(area.x), h),
        theme.base,
    );
    fill_bg(
        frame,
        Rect::new(right, y, area_right.saturating_sub(right), h),
        theme.base,
    );

    // 收缩边框(只画线,不填内部 → 保留框内内容);收到太小画不出框就只剩清屏。
    if w >= 2 && h >= 2 {
        let block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme.accent));
        frame.render_widget(block, Rect::new(x, y, w, h));
    }
}

/// 把 `rect` 清成纯 `color` 背景(先 `Clear` 去掉旧字符,再铺底色)。空矩形跳过。
fn fill_bg(frame: &mut Frame<'_>, rect: Rect, color: Color) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    frame.render_widget(Clear, rect);
    frame.render_widget(Block::new().style(Style::new().bg(color)), rect);
}

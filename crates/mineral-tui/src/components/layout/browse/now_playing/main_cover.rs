//! now_playing 面板主封面槽的单一源:内区纵切几何 + 当前主封面身份。
//! track / playlist 绘制与 page morph 封面飞行层共用,保证飞行端点与面板实画零漂移。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, Borders};

/// now_playing 内区(去边框)纵切三段:上 cover / 中 2 行 KV / 底 1 行。内区画不下
/// (过窄 / 过矮)为 `None`,与面板绘制的早退同一阈值。
///
/// # Params:
///   - `area`: 面板整区(含边框)
///
/// # Return:
///   `[cover, kv, 底行]`;画不下为 `None`。
pub(crate) fn sections(area: Rect) -> Option<[Rect; 3]> {
    let inner = Block::new().borders(Borders::ALL).inner(area);
    if inner.height < 4 || inner.width < 8 {
        return None;
    }
    Some(
        Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .areas(inner),
    )
}

//! 主界面布局计算 — 完全响应式,不写死字符尺寸。

use ratatui::layout::{Constraint, Layout, Rect};

/// 当前布局模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    /// 完整 6-面板布局(top status / left / right / transport / viz / cmd bar)。
    Full,
    /// 紧凑模式(终端过小):仅 top status / left / transport / cmd bar。
    Compact,
}

/// 主界面所有面板的位置矩形。
///
/// 阶段 2 只读 `mode` 决定后续阶段的分支(暂未消费),容忍 dead-code。
#[allow(dead_code)] // reason: `mode` 字段在阶段 6 起用于焦点路由分支
#[derive(Clone, Copy, Debug)]
pub struct Areas {
    /// 实际选用的布局模式。
    pub mode: LayoutMode,
    /// 顶部状态行(1 行)。
    pub top_status: Rect,
    /// 左栏(playlists / library)。
    pub left: Rect,
    /// 右栏(now playing detail) — Compact 模式下为 `None`。
    pub right: Option<Rect>,
    /// 底部左侧 transport 面板。
    pub transport: Rect,
    /// 底部右侧可视化区(spectrum + lyrics) — Compact 模式下为 `None`。
    pub viz: Option<Rect>,
    /// 底部命令 / 帮助行(1 行)。
    pub cmd_bar: Rect,
}

/// 紧凑模式触发宽度阈值。
const MIN_FULL_WIDTH: u16 = 80;
/// 紧凑模式触发高度阈值。
const MIN_FULL_HEIGHT: u16 = 24;

/// 按当前可用 [`Rect`] 计算各面板位置。
pub fn compute(area: Rect) -> Areas {
    if area.width < MIN_FULL_WIDTH || area.height < MIN_FULL_HEIGHT {
        compute_compact(area)
    } else {
        compute_full(area)
    }
}

fn compute_full(area: Rect) -> Areas {
    let [top_status, body, cmd_bar] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let [main_area, bottom_area] =
        Layout::vertical([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(body);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(68), Constraint::Percentage(32)])
            .areas(main_area);

    let [transport, viz] =
        Layout::horizontal([Constraint::Percentage(55), Constraint::Percentage(45)])
            .areas(bottom_area);

    Areas {
        mode: LayoutMode::Full,
        top_status,
        left,
        right: Some(right),
        transport,
        viz: Some(viz),
        cmd_bar,
    }
}

fn compute_compact(area: Rect) -> Areas {
    let [top_status, body, cmd_bar] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let [left, transport] =
        Layout::vertical([Constraint::Percentage(70), Constraint::Percentage(30)]).areas(body);

    Areas {
        mode: LayoutMode::Compact,
        top_status,
        left,
        right: None,
        transport,
        viz: None,
        cmd_bar,
    }
}

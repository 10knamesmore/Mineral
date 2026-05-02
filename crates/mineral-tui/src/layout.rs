//! 主界面布局计算 — 完全响应式,不写死字符尺寸。

use ratatui::layout::{Constraint, Layout, Rect};

/// 当前布局模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutMode {
    /// 完整布局:左半 lyrics、右半 spectrum(上)+ transport(下),顶部 + 底部状态行。
    Full,
    /// 紧凑模式(终端过小):仅 top status / left / transport / status bar。
    Compact,
}

/// 主界面所有面板的位置矩形。
#[allow(dead_code)] // reason: `mode` 字段保留给后续焦点路由分支
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
    /// 底部左半:lyrics 面板,占满 bottom 全高 — Compact 模式下为 `None`。
    pub lyrics: Option<Rect>,
    /// 底部右半上:spectrum 可视化 — Compact 模式下为 `None`。
    pub spectrum: Option<Rect>,
    /// 底部右半下(Full)/ 底部全宽(Compact):transport 进度条。
    pub transport: Rect,
    /// 底部 keys hint / 临时 hint / 搜索输入行(1 行)。
    pub status_bar: Rect,
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
    let [top_status, body, status_bar] = Layout::vertical([
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

    let [lyrics, right_col] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
            .areas(bottom_area);

    // 右半上下:spectrum 上、transport 下(transport ~7 行内容,固定高度优先,余给 spectrum)。
    let [spectrum, transport] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(7)]).areas(right_col);

    Areas {
        mode: LayoutMode::Full,
        top_status,
        left,
        right: Some(right),
        lyrics: Some(lyrics),
        spectrum: Some(spectrum),
        transport,
        status_bar,
    }
}

fn compute_compact(area: Rect) -> Areas {
    let [top_status, body, status_bar] = Layout::vertical([
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
        lyrics: None,
        spectrum: None,
        transport,
        status_bar,
    }
}

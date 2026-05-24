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

/// Full 布局:顶部 1 行状态 + 中部 60/40 主-下,主区 68/32 左-右,下方 50/50 歌词-(频谱+transport),底部 1 行 status_bar。
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

    // 右半上下:spectrum 上、transport 下(transport 内容固定 6 行 + 边框 2 = 8,余给 spectrum)。
    let [spectrum, transport] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(8)]).areas(right_col);

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

/// Compact 布局(窄/矮终端):顶/底各 1 行,中间 70/30 主-transport,无右侧歌词与频谱。
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

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{LayoutMode, compute};

    /// 造一个左上角原点、给定宽高的 area。
    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    /// 宽高都达标 → Full,right/lyrics/spectrum 都有,顶/底各 1 行。
    #[test]
    fn full_layout_above_thresholds() {
        let a = compute(area(100, 40));
        assert_eq!(a.mode, LayoutMode::Full);
        assert!(a.right.is_some());
        assert!(a.lyrics.is_some());
        assert!(a.spectrum.is_some());
        assert_eq!(a.top_status.height, 1);
        assert_eq!(a.status_bar.height, 1);
    }

    /// 恰好 80x24(阈值下界)仍是 Full。
    #[test]
    fn boundary_80x24_is_full() {
        assert_eq!(compute(area(80, 24)).mode, LayoutMode::Full);
    }

    /// 宽 < 80 → Compact,right/lyrics/spectrum 全 None。
    #[test]
    fn narrow_is_compact() {
        let a = compute(area(79, 40));
        assert_eq!(a.mode, LayoutMode::Compact);
        assert!(a.right.is_none());
        assert!(a.lyrics.is_none());
        assert!(a.spectrum.is_none());
    }

    /// 高 < 24 → Compact。
    #[test]
    fn short_is_compact() {
        assert_eq!(compute(area(100, 23)).mode, LayoutMode::Compact);
    }

    use proptest::prelude::proptest;

    proptest! {
        /// 任意尺寸:所有子区域都落在父 area 内(不越界 / 不 panic),且 Full/Compact 选择
        /// 严格按 80×24 阈值。
        #[test]
        fn areas_fit_parent_and_mode_matches(w in 0u16..=600, h in 0u16..=600) {
            let parent = area(w, h);
            let a = compute(parent);
            proptest::prop_assert_eq!(a.mode == LayoutMode::Full, w >= 80 && h >= 24);
            let fits = |c: Rect| {
                c.x >= parent.x
                    && c.y >= parent.y
                    && c.right() <= parent.right()
                    && c.bottom() <= parent.bottom()
            };
            let rects = [
                Some(a.top_status),
                Some(a.left),
                a.right,
                a.lyrics,
                a.spectrum,
                Some(a.transport),
                Some(a.status_bar),
            ];
            for r in rects.into_iter().flatten() {
                proptest::prop_assert!(fits(r), "子区域 {:?} 越出父 {:?}", r, parent);
            }
        }
    }
}

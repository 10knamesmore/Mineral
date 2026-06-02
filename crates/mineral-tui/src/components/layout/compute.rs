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

    /// 独立封面面板矩形。常规 Full/Compact 由 now_playing 内部画封面,此处仅作**全屏形变的
    /// 起点锚点**(封面从此格脱出);全屏布局为左列上方(transport 在其下)。Compact 无锚点为 `None`。
    pub cover: Option<Rect>,
    /// 底部左半:lyrics 面板,占满 bottom 全高 — Compact 模式下为 `None`。
    pub lyrics: Option<Rect>,
    /// 底部右半上:spectrum 可视化 — Compact 模式下为 `None`。
    pub spectrum: Option<Rect>,
    /// 底部右半下(Full)/ 底部全宽(Compact):transport 进度条。
    pub transport: Rect,
}

/// 紧凑模式触发宽度阈值。
const MIN_FULL_WIDTH: u16 = 80;
/// 紧凑模式触发高度阈值。
const MIN_FULL_HEIGHT: u16 = 24;

/// 全屏布局左列(cover 上 + transport 下)占宽百分比;歌词占右列剩余。
const FS_LEFT_PCT: u16 = 44;
/// 全屏布局底部 spectrum 通栏高度(比常规略高,留足频谱动态)。
const FS_SPECTRUM_HEIGHT: u16 = 10;
/// 全屏布局 transport 条高度(内容 6 行 + 边框 2,同 Full 布局);置于左列 cover 之下。
const FS_TRANSPORT_HEIGHT: u16 = 8;

/// 按当前可用 [`Rect`] 计算各面板位置。
pub fn compute(area: Rect) -> Areas {
    if area.width < MIN_FULL_WIDTH || area.height < MIN_FULL_HEIGHT {
        compute_compact(area)
    } else {
        compute_full(area)
    }
}

/// Full 布局:顶部 1 行状态 + 中部 60/40 主-下,主区 68/32 左-右,下方 50/50 歌词-(频谱+transport)。
fn compute_full(area: Rect) -> Areas {
    let [top_status, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

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
        // 全屏形变锚点:封面从右栏 now_playing 整块脱出(programmatic cover 在 t=0 盖住该块,
        // 随即飞向左半)。取整块 right 作起点,无需耦合 now_playing 内部子布局。
        cover: Some(right),
        lyrics: Some(lyrics),
        spectrum: Some(spectrum),
        transport,
    }
}

/// Compact 布局(窄/矮终端):顶部 1 行,中间 70/30 主-transport,无右侧歌词与频谱。
fn compute_compact(area: Rect) -> Areas {
    let [top_status, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

    let [left, transport] =
        Layout::vertical([Constraint::Percentage(70), Constraint::Percentage(30)]).areas(body);

    Areas {
        mode: LayoutMode::Compact,
        top_status,
        left,
        right: None,
        cover: None,
        lyrics: None,
        spectrum: None,
        transport,
    }
}

/// 全屏播放布局:左列上 cover、下 transport;右列 lyrics 通 body 全高;spectrum 全宽贴底
/// (略高)。消失面板(top_status / left / right)退化为零面积端点(形变退场用)。
///
/// ```text
/// [ cover    ][        ]
/// [----------][ lyrics ]
/// [ transport][        ]
/// [     spectrum       ]
/// ```
pub fn compute_fullscreen(area: Rect) -> Areas {
    let [body, spectrum] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(FS_SPECTRUM_HEIGHT)]).areas(area);

    let [left_col, lyrics] = Layout::horizontal([
        Constraint::Percentage(FS_LEFT_PCT),
        Constraint::Percentage(100 - FS_LEFT_PCT),
    ])
    .areas(body);

    let [cover, transport] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(FS_TRANSPORT_HEIGHT)])
            .areas(left_col);

    let zero = Rect::new(area.x, area.y, 0, 0);
    Areas {
        mode: LayoutMode::Full,
        top_status: zero,
        left: zero,
        right: Some(zero),
        cover: Some(cover),
        lyrics: Some(lyrics),
        spectrum: Some(spectrum),
        transport,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{LayoutMode, compute, compute_fullscreen};

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

    /// 全屏布局:左列上 cover / 下 transport,右列 lyrics 通高,spectrum 全宽贴底,消失面板零面积。
    #[test]
    fn fullscreen_layout_panels() -> color_eyre::Result<()> {
        let a = compute_fullscreen(area(100, 40));
        let cover = a
            .cover
            .ok_or_else(|| color_eyre::eyre::eyre!("全屏缺 cover"))?;
        let lyrics = a
            .lyrics
            .ok_or_else(|| color_eyre::eyre::eyre!("全屏缺 lyrics"))?;
        let spectrum = a
            .spectrum
            .ok_or_else(|| color_eyre::eyre::eyre!("全屏缺 spectrum"))?;
        let transport = a.transport;

        // 左列在左、lyrics 在右,无缝相接。
        assert!(cover.x < lyrics.x, "cover 在左列、lyrics 在右列");
        assert_eq!(cover.right(), lyrics.x, "cover 右缘接 lyrics 左缘");
        assert_eq!(cover.y, lyrics.y, "cover / lyrics 顶对齐");

        // 左列:transport 在 cover 之下,同左对齐、右缘同接 lyrics。
        assert_eq!(transport.x, cover.x, "transport 与 cover 同左");
        assert_eq!(
            transport.right(),
            lyrics.x,
            "transport 右缘接 lyrics(同在左列)"
        );
        assert!(transport.y >= cover.bottom(), "transport 在 cover 之下");

        // lyrics 通 body 全高:底与左列底(transport 底)对齐。
        assert_eq!(lyrics.bottom(), transport.bottom(), "lyrics 通 body 全高");

        // spectrum 全宽贴底、在 body 之下。
        assert_eq!(spectrum.width, 100, "spectrum 通栏全宽");
        assert!(spectrum.y >= transport.bottom(), "spectrum 在左列之下");
        assert!(spectrum.y >= lyrics.bottom(), "spectrum 在 lyrics 之下");

        assert_eq!(a.top_status.height, 0, "全屏顶栏零高");
        assert_eq!(a.left.width, 0, "全屏左栏零宽");
        Ok(())
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
            ];
            for r in rects.into_iter().flatten() {
                proptest::prop_assert!(fits(r), "子区域 {:?} 越出父 {:?}", r, parent);
            }
        }

        /// 全屏布局任意尺寸下子区都落在父 area 内(不越界 / 不 panic)。
        #[test]
        fn fullscreen_areas_fit_parent(w in 0u16..=600, h in 0u16..=600) {
            let parent = area(w, h);
            let a = compute_fullscreen(parent);
            let fits = |c: Rect| {
                c.x >= parent.x
                    && c.y >= parent.y
                    && c.right() <= parent.right()
                    && c.bottom() <= parent.bottom()
            };
            for r in [a.cover, a.lyrics, a.spectrum].into_iter().flatten() {
                proptest::prop_assert!(fits(r), "全屏子区 {:?} 越出父 {:?}", r, parent);
            }
            proptest::prop_assert!(fits(a.transport), "transport {:?} 越出父 {:?}", a.transport, parent);
        }
    }
}

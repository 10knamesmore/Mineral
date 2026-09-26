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
#[derive(Clone, Copy, Debug)]
pub struct Areas {
    /// 实际选用的布局模式。
    pub mode: LayoutMode,

    /// 顶部状态行(1 行)。
    pub top_status: Rect,

    /// 左栏(playlists / library) — search 端点复用为结果(results)列。
    pub left: Rect,

    /// 右栏(now playing detail) — Compact 模式下为 `None`。search 端点复用为详情(detail)面板。
    pub right: Option<Rect>,

    /// token prompt 输入行(3 行带边框,顶栏下全宽)。仅 search 布局端点为 `Some`;normal /
    /// fullscreen 端点为 `None`(同 `cover` 一样按端点取舍的锚点 `Option`)。
    pub search_prompt: Option<Rect>,

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

/// 按当前可用 [`Rect`] 计算各面板位置。
///
/// # Params:
///   - `area`: 可用区域
///   - `cfg`: 布局段(完整布局门槛与播放栏高度，配置 `tui.layout`)
pub fn compute(area: Rect, cfg: &mineral_config::LayoutConfig) -> Areas {
    if area.width < *cfg.min_full_width() || area.height < *cfg.min_full_height() {
        compute_compact(area, cfg)
    } else {
        compute_full(area, cfg)
    }
}

/// Full 布局:顶部 1 行状态 + 中部 60/40 主-下,主区 68/32 左-右,下方 50/50 歌词-(频谱+transport)。
fn compute_full(area: Rect, cfg: &mineral_config::LayoutConfig) -> Areas {
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

    // 右半上下:播放栏从配置现读高度，其余留给频谱。
    let [spectrum, transport] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(*cfg.transport_height()),
    ])
    .areas(right_col);

    Areas {
        mode: LayoutMode::Full,
        top_status,
        left,
        right: Some(right),
        search_prompt: None,
        // 全屏形变锚点:封面从右栏 now_playing 整块脱出(programmatic cover 在 t=0 盖住该块,
        // 随即飞向左半)。取整块 right 作起点,无需耦合 now_playing 内部子布局。
        cover: Some(right),
        lyrics: Some(lyrics),
        spectrum: Some(spectrum),
        transport,
    }
}

/// Compact 布局(窄/矮终端):顶部 1 行，播放栏从底部占配置高度，其余归左栏。
fn compute_compact(area: Rect, cfg: &mineral_config::LayoutConfig) -> Areas {
    let [top_status, body] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

    let [left, transport] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(*cfg.transport_height()),
    ])
    .areas(body);

    Areas {
        mode: LayoutMode::Compact,
        top_status,
        left,
        right: None,
        search_prompt: None,
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
pub fn compute_fullscreen(area: Rect, cfg: &mineral_config::LayoutConfig) -> Areas {
    let [body, spectrum] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(cfg.fs_spectrum().resolve(area.height)),
    ])
    .areas(area);

    let left_pct = (*cfg.fs_left_pct()).min(100);
    let [left_col, lyrics] = Layout::horizontal([
        Constraint::Percentage(left_pct),
        Constraint::Percentage(100 - left_pct),
    ])
    .areas(body);

    let [cover, transport] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(*cfg.transport_height()),
    ])
    .areas(left_col);

    let zero = Rect::new(area.x, area.y, 0, 0);
    Areas {
        mode: LayoutMode::Full,
        top_status: zero,
        left: zero,
        right: Some(zero),
        search_prompt: None,
        cover: Some(cover),
        lyrics: Some(lyrics),
        spectrum: Some(spectrum),
        transport,
    }
}

/// Search 布局端点(继 [`compute`] / [`compute_fullscreen`] 之后的第三个):**prompt 框接管顶行**
/// ——不留 status bar(其信息在 search 态无意义),3 行 token prompt 全宽贴 area 顶(带边框,焦点
/// 高亮),主体左 results(38%)右 detail(62%)的 master-detail,transport 全宽贴底。top_status
/// 退化零面积、cover / lyrics / spectrum 在此端点无锚均为 `None`。
///
/// # Params:
///   - `area`: 可用区域
///   - `cfg`: 布局段，播放栏高度与其他布局计算函数共用
pub fn compute_search(area: Rect, cfg: &mineral_config::LayoutConfig) -> Areas {
    // search 态顶行被 prompt 接管:不留 status bar,prompt 紧贴 area 顶。
    let [search_prompt, body] =
        Layout::vertical([Constraint::Length(3), Constraint::Min(0)]).areas(area);

    let [results_body, transport] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(*cfg.transport_height()),
    ])
    .areas(body);

    let [results, detail] =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
            .areas(results_body);

    Areas {
        mode: LayoutMode::Full,
        // top_status 退化零面积:morph 里 browse 顶栏朝中心收掉,稳态 search 无 status bar。
        top_status: Rect::new(area.x, area.y, 0, 0),
        left: results,
        right: Some(detail),
        search_prompt: Some(search_prompt),
        cover: None,
        lyrics: None,
        spectrum: None,
        transport,
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{compute, compute_fullscreen, compute_search};

    /// 造一个左上角原点、给定宽高的 area。
    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    /// default.lua 的布局段。
    fn layout_cfg() -> color_eyre::Result<mineral_config::LayoutConfig> {
        Ok(mineral_config::Config::defaults()?.tui().layout().clone())
    }

    /// 可用面积不足时播放栏收缩，各端点的矩形不越界也不 panic。
    #[test]
    fn tiny_areas_keep_all_panels_inside_parent() -> color_eyre::Result<()> {
        let cfg = layout_cfg()?;
        for (width, height) in [(0, 0), (0, 10), (10, 0), (1, 1), (5, 4), (79, 4), (100, 1)] {
            let parent = Rect::new(3, 2, width, height);
            for panels in [
                compute(parent, &cfg),
                compute_fullscreen(parent, &cfg),
                compute_search(parent, &cfg),
            ] {
                for rect in [
                    Some(panels.top_status),
                    Some(panels.left),
                    panels.right,
                    panels.search_prompt,
                    panels.cover,
                    panels.lyrics,
                    panels.spectrum,
                    Some(panels.transport),
                ]
                .into_iter()
                .flatten()
                {
                    assert!(
                        rect.x >= parent.x
                            && rect.y >= parent.y
                            && rect.right() <= parent.right()
                            && rect.bottom() <= parent.bottom(),
                        "子区域 {rect:?} 越出父 {parent:?}"
                    );
                }
                assert!(panels.transport.height <= height.min(*cfg.transport_height()));
            }
        }
        Ok(())
    }

    use proptest::prelude::proptest;

    proptest! {
        /// 任意尺寸:所有子区域都落在父 area 内(不越界 / 不 panic)。
        #[test]
        fn areas_fit_parent(w in 0u16..=600, h in 0u16..=600) {
            let Ok(cfg) = layout_cfg() else {
                return Err(proptest::test_runner::TestCaseError::fail("defaults 不可用"));
            };
            let parent = area(w, h);
            let a = compute(parent, &cfg);
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
            let Ok(cfg) = layout_cfg() else {
                return Err(proptest::test_runner::TestCaseError::fail("defaults 不可用"));
            };
            let parent = area(w, h);
            let a = compute_fullscreen(parent, &cfg);
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

        /// search 布局任意尺寸下子区都落在父 area 内(不越界 / 不 panic)。
        #[test]
        fn search_areas_fit_parent(w in 0u16..=600, h in 0u16..=600) {
            let Ok(cfg) = layout_cfg() else {
                return Err(proptest::test_runner::TestCaseError::fail("defaults 不可用"));
            };
            let parent = area(w, h);
            let a = compute_search(parent, &cfg);
            let fits = |c: Rect| {
                c.x >= parent.x
                    && c.y >= parent.y
                    && c.right() <= parent.right()
                    && c.bottom() <= parent.bottom()
            };
            for r in [a.search_prompt, a.right].into_iter().flatten() {
                proptest::prop_assert!(fits(r), "search 子区 {:?} 越出父 {:?}", r, parent);
            }
            proptest::prop_assert!(fits(a.left), "results {:?} 越出父 {:?}", a.left, parent);
            proptest::prop_assert!(fits(a.transport), "transport {:?} 越出父 {:?}", a.transport, parent);
        }
    }
}

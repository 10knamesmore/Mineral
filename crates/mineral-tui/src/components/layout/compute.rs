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
    /// 左栏(playlists / library) — search 端点复用为结果(results)列。
    pub left: Rect,
    /// 右栏(now playing detail) — Compact 模式下为 `None`。search 端点复用为详情(detail)面板。
    pub right: Option<Rect>,

    /// token prompt 输入行(1 行,顶栏下全宽)。仅 search 布局端点为 `Some`;normal /
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
///   - `cfg`: 布局段(完整布局门槛,配置 `tui.layout`)
pub fn compute(area: Rect, cfg: &mineral_config::LayoutConfig) -> Areas {
    if area.width < *cfg.min_full_width() || area.height < *cfg.min_full_height() {
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
        search_prompt: None,
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
        Constraint::Length(*cfg.fs_spectrum_height()),
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
        Constraint::Length(*cfg.fs_transport_height()),
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

/// Search 布局端点(继 [`compute`] / [`compute_fullscreen`] 之后的第三个):顶栏保留,其下
/// 1 行 token prompt 全宽,主体左 results(38%)右 detail(62%)的 master-detail,transport
/// 全宽贴底。退场面板 cover / lyrics / spectrum 在此端点无锚为 `None`。
///
/// # Params:
///   - `area`: 可用区域
///   - `_cfg`: 布局段;暂未读,保留入参令三个布局端点签名一致(备后续响应式宽度比)
pub fn compute_search(area: Rect, _cfg: &mineral_config::LayoutConfig) -> Areas {
    let [top_status, search_prompt, body] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(area);

    // transport 高度沿用 normal(compute_full):内容固定 6 行 + 边框 2 = 8。
    let [results_body, transport] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(8)]).areas(body);

    let [results, detail] =
        Layout::horizontal([Constraint::Percentage(38), Constraint::Percentage(62)])
            .areas(results_body);

    Areas {
        mode: LayoutMode::Full,
        top_status,
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

    use super::{LayoutMode, compute, compute_fullscreen, compute_search};

    /// 造一个左上角原点、给定宽高的 area。
    fn area(w: u16, h: u16) -> Rect {
        Rect::new(0, 0, w, h)
    }

    /// defaults 配置的布局段(= 接线前硬编码阈值/尺寸)。
    fn layout_cfg() -> color_eyre::Result<mineral_config::LayoutConfig> {
        Ok(mineral_config::Config::defaults()?.tui().layout().clone())
    }

    /// 宽高都达标 → Full,right/lyrics/spectrum 都有,顶/底各 1 行。
    #[test]
    fn full_layout_above_thresholds() -> color_eyre::Result<()> {
        let cfg = layout_cfg()?;
        let a = compute(area(100, 40), &cfg);
        assert_eq!(a.mode, LayoutMode::Full);
        assert!(a.right.is_some());
        assert!(a.lyrics.is_some());
        assert!(a.spectrum.is_some());
        assert_eq!(a.top_status.height, 1);
        Ok(())
    }

    /// 恰好 80x24(阈值下界)仍是 Full。
    #[test]
    fn boundary_80x24_is_full() -> color_eyre::Result<()> {
        let cfg = layout_cfg()?;
        assert_eq!(compute(area(80, 24), &cfg).mode, LayoutMode::Full);
        Ok(())
    }

    /// 宽 < 80 → Compact,right/lyrics/spectrum 全 None。
    #[test]
    fn narrow_is_compact() -> color_eyre::Result<()> {
        let cfg = layout_cfg()?;
        let a = compute(area(79, 40), &cfg);
        assert_eq!(a.mode, LayoutMode::Compact);
        assert!(a.right.is_none());
        assert!(a.lyrics.is_none());
        assert!(a.spectrum.is_none());
        Ok(())
    }

    /// 高 < 24 → Compact。
    #[test]
    fn short_is_compact() -> color_eyre::Result<()> {
        let cfg = layout_cfg()?;
        assert_eq!(compute(area(100, 23), &cfg).mode, LayoutMode::Compact);
        Ok(())
    }

    /// 全屏布局:左列上 cover / 下 transport,右列 lyrics 通高,spectrum 全宽贴底,消失面板零面积。
    #[test]
    fn fullscreen_layout_panels() -> color_eyre::Result<()> {
        let cfg = layout_cfg()?;
        let a = compute_fullscreen(area(100, 40), &cfg);
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

    /// search 布局:顶栏下 prompt 行全宽,results 在左、detail 在右无缝相接,transport 全宽贴底,
    /// lyrics/spectrum/cover 退场(None)。
    #[test]
    fn search_layout_panels() -> color_eyre::Result<()> {
        let cfg = layout_cfg()?;
        let parent = area(100, 40);
        let a = compute_search(parent, &cfg);

        let prompt = a
            .search_prompt
            .ok_or_else(|| color_eyre::eyre::eyre!("search 缺 prompt 行"))?;
        let detail = a
            .right
            .ok_or_else(|| color_eyre::eyre::eyre!("search 缺 detail(right)"))?;
        let results = a.left;
        let transport = a.transport;

        // 顶栏 1 行不动,prompt 紧贴其下、全宽 1 行。
        assert_eq!(a.top_status.height, 1, "顶栏保留 1 行");
        assert_eq!(prompt.y, a.top_status.bottom(), "prompt 紧接顶栏下");
        assert_eq!(prompt.height, 1, "prompt 1 行");
        assert_eq!(prompt.width, parent.width, "prompt 全宽");

        // results 在左、detail 在右,无缝相接、顶对齐。
        assert!(results.x < detail.x, "results 在左、detail 在右");
        assert_eq!(results.right(), detail.x, "results 右缘接 detail 左缘");
        assert_eq!(results.y, detail.y, "results / detail 顶对齐");
        assert!(results.y >= prompt.bottom(), "body 在 prompt 之下");

        // transport 全宽贴底,在 body 之下。
        assert_eq!(transport.width, parent.width, "transport 通栏全宽");
        assert_eq!(transport.bottom(), parent.bottom(), "transport 贴底");
        assert!(transport.y >= results.bottom(), "transport 在 results 之下");
        assert!(transport.y >= detail.bottom(), "transport 在 detail 之下");

        // 退场面板在 search 端点无锚。
        assert!(a.lyrics.is_none(), "search 端点 lyrics 退场");
        assert!(a.spectrum.is_none(), "search 端点 spectrum 退场");
        assert!(a.cover.is_none(), "search 端点 cover 退场");

        Ok(())
    }

    use proptest::prelude::proptest;

    proptest! {
        /// 任意尺寸:所有子区域都落在父 area 内(不越界 / 不 panic),且 Full/Compact 选择
        /// 严格按 80×24 阈值。
        #[test]
        fn areas_fit_parent_and_mode_matches(w in 0u16..=600, h in 0u16..=600) {
            let Ok(cfg) = layout_cfg() else {
                return Err(proptest::test_runner::TestCaseError::fail("defaults 不可用"));
            };
            let parent = area(w, h);
            let a = compute(parent, &cfg);
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

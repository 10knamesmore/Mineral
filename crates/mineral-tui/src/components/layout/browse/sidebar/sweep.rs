//! 左栏 Playlists ↔ Library 切换的横向过渡合成。
//!
//! 两个视图各渲染到一块和左栏等大的离屏 [`Buffer`],再按过渡进度把列搬运 / 拼接进目标
//! 区域。`Push` 让两块一起平移、`Cover` 让新视图从右覆盖旧视图。过渡风格由配置
//! `tui.animation.view_sweep` 选定([`SweepStyle`],调用方从 `state.cfg` 取传入)。

use mineral_config::SweepStyle;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;

use super::{library, playlists};
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 缓动进度满值(千分比),对齐 [`crate::render::anim::Transition::eased`]。
const FULL: u32 = 1000;

/// 把 Playlists 与 Library 两视图按 `eased`(缓动千分比)横向合成到 `area`。
///
/// 仅在过渡中途调用(进度既非起点也非终点);端点退化为单视图由 [`super::draw`] 直接画。
///
/// # Params:
///   - `buf`: 目标(屏幕)缓冲
///   - `area`: 左栏区域
///   - `state`: 应用状态(两视图渲染所需)
///   - `theme`: 主题
///   - `eased`: 缓动后的过渡进度,`0` = 全 Playlists、满值 = 全 Library
///   - `style`: 过渡风格(Push / Cover)
pub fn draw(
    buf: &mut Buffer,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    eased: u16,
    style: SweepStyle,
) {
    // 两视图各渲染到等大离屏 buffer(坐标系与屏幕一致,含 area 的 x/y 偏移)。
    let mut pl = Buffer::empty(area);
    let mut lib = Buffer::empty(area);
    playlists::render_to(&mut pl, area, state, theme);
    library::render_to(&mut lib, area, state, theme);

    let w = area.width;
    // Library 已「进入」的列数(0..=w)。
    let advance = u16::try_from(u32::from(w) * u32::from(eased) / FULL)
        .unwrap_or(w)
        .min(w);

    for c in 0..w {
        let (src, src_c) = match style {
            // 旧视图不动;新视图占据最右 advance 列(其左边框落在分界列 = 覆盖左缘)。
            SweepStyle::Cover => {
                let split = w - advance;
                if c < split {
                    (&pl, c)
                } else {
                    (&lib, c - split)
                }
            }
            // 旧视图整体左移 advance,新视图从右补入。
            // SweepStyle 是 #[non_exhaustive];本接线不识别的变体按 Push 处理。
            SweepStyle::Push | _ => {
                if c + advance < w {
                    (&pl, c + advance)
                } else {
                    (&lib, c + advance - w)
                }
            }
        };
        copy_col(buf, area, src, c, src_c);
    }
}

/// 把离屏 `src` 的第 `src_c` 列(相对 `area`)整列搬到目标 `dst` 的第 `dst_c` 列。
///
/// # Params:
///   - `dst`: 目标缓冲
///   - `area`: 列所在区域(提供 x/y 偏移与行高)
///   - `src`: 离屏源缓冲
///   - `dst_c`: 目标相对列号
///   - `src_c`: 源相对列号
fn copy_col(dst: &mut Buffer, area: Rect, src: &Buffer, dst_c: u16, src_c: u16) {
    let dx = area.x + dst_c;
    let sx = area.x + src_c;
    for ry in area.y..area.y.saturating_add(area.height) {
        if let Some(cell) = src.cell((sx, ry)) {
            let mut cell = cell.clone();
            // 离屏帧空 cell 的 Reset 底视作透明:回落到目标已铺的 backdrop 底(paint_backdrop
            // 铺的 theme.background + 氛围场)。否则整格搬回会把 backdrop 盖成终端默认底,过场
            // 期腾出/空白列露出终端底洞——稳态面板背景全靠底层 backdrop,离屏合成必须让它透出。
            if matches!(cell.bg, Color::Reset)
                && let Some(under) = dst.cell((dx, ry))
            {
                cell.set_bg(under.bg);
            }
            if let Some(slot) = dst.cell_mut((dx, ry)) {
                *slot = cell;
            }
        }
    }
}

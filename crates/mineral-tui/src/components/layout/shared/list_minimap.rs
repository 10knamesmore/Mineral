//! 在曲目列表、歌单列表和队列的右边框上绘制位置缩略图，展示喜欢、在播和缓动光标。
//!
//! 调用方提供过滤后的序号与本帧光标输入（见 [`MinimapCursor`]）；每帧按轨道高度合并标记。
//! 光标位置与吸附进度都是列表级的跨帧动画，本模块只读取与推进，不自己持有。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::symbols::braille;

use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::scroll::position::{
    MagnetProgress, POSITION_SCALE, boundary_position, relative_position,
};

/// 盲文字符每格的纵向点数，光标以四分之一格移动。
const DOT_ROWS_PER_CELL: u64 = 4;

/// 右列四点组成轨道，整格颜色兼作喜欢标记；左列只放光标点。
const TRACK_DOTS: u8 = 0xB8;

/// 本帧 minimap 的输入：位置与吸附进度是列表级动画，几何旋钮每帧现读配置。
#[derive(Clone, Copy)]
pub(crate) struct MinimapCursor<'a> {
    /// 光标在整份列表中的归一化位置，范围为 `0..=POSITION_SCALE`；`None` = 没有光标。
    pub(crate) position: Option<u32>,

    /// 吸附进度：`0` = 在播色 + 光晕照常，满值 = 光标色 + 光晕熄灭。
    pub(crate) magnet: &'a MagnetProgress,

    /// 从配置折算的光标移动 / 吸附过渡拍数。
    pub(crate) ticks: u16,

    /// 稳态实拍推进动画；离屏合成与形变帧只读采样。
    pub(crate) advancing: bool,

    /// 最小光晕半径（轨道行数），与条目平分范围取较大值。
    pub(crate) halo_rows: u64,

    /// 吸附区半径
    pub(crate) magnet_dots: u64,
}

impl<'a> MinimapCursor<'a> {
    /// 从列表的光标动画、本帧渲染模式与 minimap 配置组装输入。
    ///
    /// # Params:
    ///   - `list`: 该列表的滚动 / 动画态；光标位置与吸附进度都从它取。
    ///   - `len`: 当前显示列表长度。
    ///   - `motion`: 稳态实拍推进一拍，离屏合成只读采样。
    ///   - `ticks`: 当前配置折算的光标移动拍数。
    ///   - `minimap`: 当前 `tui.minimap` 段（现读，热更即生效）。
    pub(crate) fn new(
        list: &'a ScrollList,
        len: usize,
        motion: ScrollMotion,
        ticks: u16,
        minimap: &mineral_config::MinimapConfig,
    ) -> Self {
        Self {
            position: list.position(len, motion, ticks),
            magnet: list.magnet(),
            ticks,
            advancing: matches!(motion, ScrollMotion::Advancing { .. }),
            halo_rows: *minimap.halo_rows(),
            magnet_dots: *minimap.magnet_dots(),
        }
    }
}

/// 一首曲目在当前过滤视图中的位置及需要显示的标记。
pub(crate) struct MinimapEntry {
    /// 从零开始的过滤视图序号；大于等于列表总数时不参与绘制。
    pub(crate) index: usize,

    /// 曲目是否已喜欢；它占的那一段轨道整格染红，不动左列。
    pub(crate) loved: bool,

    /// 曲目是否在播；所在格显示菱形，同时是光标的吸附目标。
    pub(crate) playing: bool,
}

/// 当前帧中压缩到同一轨道行的曲目标记，通过逻辑或合并，结果与输入顺序无关。
#[derive(Clone, Copy, Default)]
struct RowMarkers {
    /// 该格是否包含已喜欢曲目。
    loved: bool,

    /// 该格是否包含在播曲目。
    playing: bool,
}

/// 把整份列表的位置与标记绘制到右边框的数据行上。
///
/// 首尾曲目分别贴住轨道首末盲文点，单曲列表贴顶。右列四点是轨道，按相邻条目的中点
/// 划分 per-item 平分区间：喜欢标记染红条目所属区间，光晕按区间计算半径，并至少扩展
/// 配置指定的行数；左列只归光标。
/// 在播格显示 `◆`：没被吸住时保持主题绿；光标点进入 `tui.minimap.magnet_dots` 个点内
/// 就被吸走——光标点不显示，`◆` 按 [`MagnetProgress`] 缓动淡向光标色，光晕同步淡出。
/// 空列表保留淡色盲文轨道。
///
/// # Params:
///   - `buf`: 已绘制面板边框的当前帧缓冲区；保留每格原有背景和字体效果。
///   - `track`: 右边框数据行的一列，不含表头和上下圆角；只在此区域内落笔。
///   - `total`: 当前过滤视图的曲目总数；为零时只绘制轨道，忽略光标和标记。
///   - `cursor`: 本帧光标输入（位置 + 吸附进度 + 推进模式）。
///   - `entries`: 需要投影的曲目；同格标记合并，越出过滤视图的序号被忽略。
///   - `theme`: 当前主题；轨道、光标、喜欢和在播分别使用 `surface1`、`accent`、`red`、`green`。
pub(crate) fn render_minimap(
    buf: &mut Buffer,
    track: Rect,
    total: usize,
    cursor: MinimapCursor<'_>,
    entries: impl Iterator<Item = MinimapEntry>,
    theme: &Theme,
) {
    if track.is_empty() {
        return;
    }

    let MinimapCursor {
        position,
        magnet,
        ticks,
        advancing,
        halo_rows,
        magnet_dots,
    } = cursor;
    let row_span = u64::from(track.height - 1);
    let dot_span = u64::from(track.height) * DOT_ROWS_PER_CELL - 1;
    let cursor_position = position
        .filter(|_| total > 0)
        .map(|position| u64::from(position) * dot_span);
    let cursor_dot = cursor_position.map(nearest_dot_row);
    let mut nearest_playing: Option<u64> = None;
    let mut rows = vec![RowMarkers::default(); usize::from(track.height)];
    for entry in entries {
        if entry.index >= total || !(entry.loved || entry.playing) {
            continue;
        }
        let Some(position) = relative_position(entry.index, total) else {
            continue;
        };
        // 先投影到盲文点再合并到字符格，保证标记与光标使用同一坐标。
        let dot = nearest_dot_row(u64::from(position) * dot_span);
        if entry.playing {
            if let Some(cursor) = cursor_dot
                && nearest_playing.is_none_or(|best| cursor.abs_diff(dot) < cursor.abs_diff(best))
            {
                nearest_playing = Some(dot);
            }
            if let Some(row) = row_index(dot)
                && let Some(markers) = rows.get_mut(row)
            {
                markers.playing = true;
            }
        }
        // 喜欢染红整项在全列表中的等分区间：相邻两项共用边界，连着的喜欢连成一片。
        if entry.loved
            && let Some((first, last)) = loved_span(entry.index, total, dot_span)
            && let Some((first, last)) = row_index(first).zip(row_index(last))
        {
            for markers in rows.iter_mut().skip(first).take(last - first + 1) {
                markers.loved = true;
            }
        }
    }

    // 光标点落进在播点的吸附区就被吸走：区内不画光标点，只留那一格的在播标记。
    let absorbed = match (cursor_dot, nearest_playing) {
        (Some(at), Some(playing)) => at.abs_diff(playing) <= magnet_dots,
        _ => false,
    };
    // 吸附的颜色是缓动过渡：在播格淡向光标色，光晕同步淡出。
    let absorb = u64::from(if advancing {
        magnet.advance(absorbed, ticks)
    } else {
        magnet.frozen()
    });
    let cursor_dot = cursor_dot.filter(|_| !absorbed);
    // 光晕在字符格中心采样；首末点超出中心范围时贴到端点，保持端点光标明亮。
    let halo_position = cursor_position.map(|position| {
        let cell_center = (DOT_ROWS_PER_CELL - 1) * u64::from(POSITION_SCALE) / 2;
        (position.saturating_sub(cell_center) / DOT_ROWS_PER_CELL)
            .min(row_span * u64::from(POSITION_SCALE))
    });
    let halo = halo_position.zip(halo_radius(total, dot_span, halo_rows));
    for (y, markers) in (track.top()..track.bottom()).zip(rows) {
        let row = u64::from(y - track.top());
        let strength = halo.map_or(0, |(position, radius)| halo_strength(row, position, radius));
        // 吸住的光晕按同一进度淡出，离开吸附区时再淡回来。
        let glow = strength * (1000 - absorb) / 1000;
        let center = cursor_dot.filter(|dot| dot / DOT_ROWS_PER_CELL == row);
        let dots = TRACK_DOTS | center.map_or(0, marker_dot);
        let symbol = if markers.playing {
            Some('◆')
        } else {
            char::from_u32(u32::from(braille::BLANK) + u32::from(dots))
        };
        // 在播格取「主题绿 → 光标色」的过渡值，不吃光晕渐变。
        let fg = if markers.playing {
            lerp_color(theme.green, theme.accent, absorb, 1000)
        } else {
            let base = if center.is_none() && markers.loved {
                theme.red
            } else {
                theme.surface1
            };
            lerp_color(base, theme.accent, glow, 1000)
        };
        if let Some(cell) = buf.cell_mut((track.x, y))
            && let Some(symbol) = symbol
        {
            cell.set_char(symbol).set_fg(fg);
        }
    }
}

/// 将定点位置舍入到最近的盲文点，恰好半点时取下一点。
///
/// # Params:
///   - `position`: 从轨道首点起算的位置，每个点距以 `POSITION_SCALE` 个单位表示。
fn nearest_dot_row(position: u64) -> u64 {
    (position + u64::from(POSITION_SCALE) / 2) / u64::from(POSITION_SCALE)
}

/// 将条目的 per-item 平分区间投影为喜欢标记覆盖的盲文点区间（闭区间）。
///
/// 第 `index` 项占它与前后邻居的中点之间那一份轨道，首末项贴住轨道两端；相邻两项因此
/// 共用一条边界，连着的喜欢在轨道上首尾相接。
///
/// # Params:
///   - `index`: 当前显示顺序中的下标，越界时钳到末项。
///   - `total`: 当前显示列表的总项数；空列表没有区间。
///   - `dot_span`: 轨道末点的序号。
fn loved_span(index: usize, total: usize, dot_span: u64) -> Option<(u64, u64)> {
    let last = total.checked_sub(1)?;
    let index = index.min(last);
    let start = boundary_position(index, total)?;
    let end = boundary_position(index + 1, total)?;
    Some((
        nearest_dot_row(u64::from(start) * dot_span),
        nearest_dot_row(u64::from(end) * dot_span),
    ))
}

/// 盲文点所属的轨道行偏移。
///
/// # Params:
///   - `dot`: 从轨道首点起算的整数点位；行数受 `u16` 限制，点位总能落在 `usize` 内。
fn row_index(dot: u64) -> Option<usize> {
    usize::try_from(dot / DOT_ROWS_PER_CELL).ok()
}

/// 把全轨道点位转成盲文左列的单点掩码，即光标在格内的精确点位。
///
/// # Params:
///   - `dot_row`: 从轨道首点起算的整数点位。
fn marker_dot(dot_row: u64) -> u8 {
    match dot_row % DOT_ROWS_PER_CELL {
        0 => 0x01,
        1 => 0x02,
        2 => 0x04,
        _ => 0x40,
    }
}

/// 按 per-item 平分区间计算光晕半径：相邻条目间距的一半，单项覆盖整条轨道。
///
/// # Params:
///   - `total`: 当前过滤视图的条目总数；空列表没有光晕。
///   - `dot_span`: 轨道末点的序号。
///   - `min_rows`: 最小半径（轨道行数）；至少一行以保持密集列表的光标可见。
///
/// # Return:
///   以 `POSITION_SCALE` 为一行的半径，保留平分后的不足一行部分。
fn halo_radius(total: usize, dot_span: u64, min_rows: u64) -> Option<u64> {
    let item_radius = u64::from(boundary_position(1, total)?) * dot_span / DOT_ROWS_PER_CELL;
    let min_radius = min_rows.max(1).saturating_mul(u64::from(POSITION_SCALE));
    Some(item_radius.max(min_radius))
}

/// 按距离计算千分比光晕强度：中心满亮，向外按二次曲线先快后慢衰减，到半径处归零。
///
/// # Params:
///   - `row`: 从轨道首行起算的整数行偏移。
///   - `position`: 光标的定点行位置，每行以 `POSITION_SCALE` 个单位表示。
///   - `radius`: [`halo_radius`] 计算的正半径，与 `position` 使用同一单位。
///
/// # Return:
///   `0..=1000` 的混色比例；保留小数行距离，使邻格颜色随光标连续变化。
fn halo_strength(row: u64, position: u64, radius: u64) -> u64 {
    let distance = (row * u64::from(POSITION_SCALE)).abs_diff(position);
    let fraction = (distance * 1000 / radius).min(1000);
    // (1 - distance / radius)^2：中心附近下降较快，越靠近边缘收尾越柔和。
    let remaining = 1000 - fraction;
    remaining * remaining / 1000
}

/// 验证压缩标记、轨道边界和逐格颜色变化的渲染约定。
#[cfg(test)]
mod tests {
    use ratatui::buffer::{Buffer, Cell};
    use ratatui::layout::{Position, Rect};
    use ratatui::style::Color;

    use super::{MagnetProgress, MinimapCursor, MinimapEntry, render_minimap};
    use crate::render::theme::Theme;
    use crate::runtime::scroll::position::POSITION_SCALE;
    use crate::test_support::default_theme;

    /// 测试用的最小光晕半径（轨道行数）。
    const HALO_ROWS: u64 = 5;

    /// 测试用的吸附区半径（盲文点）。
    const MAGNET_DOTS: u64 = 2;

    /// 读取必须存在的测试格；坐标错误时让测试返回带坐标的错误。
    ///
    /// # Params:
    ///   - `buf`: 待检查的渲染结果。
    ///   - `x`: 格子的绝对列坐标。
    ///   - `y`: 格子的绝对行坐标。
    fn cell(buf: &Buffer, x: u16, y: u16) -> color_eyre::Result<&Cell> {
        buf.cell((x, y))
            .ok_or_else(|| color_eyre::eyre::eyre!("测试格 ({x}, {y}) 不在缓冲区内"))
    }

    /// 固定位置渲染一帧；吸附状态每帧新建，直接落在目标值。
    ///
    /// 跨帧的过渡断言用 [`render_minimap`]，两者差别只在吸附状态是否跨帧。
    ///
    /// # Params:
    ///   - `buf`: 目标缓冲区。
    ///   - `track`: 轨道区域。
    ///   - `total`: 列表总数。
    ///   - `cursor`: 归一化光标位置。
    ///   - `entries`: 需要投影的标记。
    ///   - `theme`: 当前主题。
    fn render_pinned(
        buf: &mut Buffer,
        track: Rect,
        total: usize,
        cursor: Option<u32>,
        entries: impl Iterator<Item = MinimapEntry>,
        theme: &Theme,
    ) {
        let cursor = MinimapCursor {
            position: cursor,
            magnet: &MagnetProgress::default(),
            ticks: 1,
            advancing: true,
            halo_rows: HALO_ROWS,
            magnet_dots: MAGNET_DOTS,
        };
        render_minimap(buf, track, total, cursor, entries, theme);
    }

    /// 压缩冲突与输入顺序无关：在播出菱形，喜欢把轨道整格染红，右列只有光标点。
    #[test]
    fn compressed_markers_keep_priority_and_show_shared_cell() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        let track = Rect::new(4, 2, 1, 5);
        let entries = [
            (0, true, false),
            (1, false, true),
            (25, true, false),
            (26, true, true),
            (50, true, false),
            (75, true, false),
            (100, true, false),
            (101, false, true),
        ];
        for reverse in [false, true] {
            let mut ordered = entries;
            if reverse {
                ordered.reverse();
            }
            let render = |cursor| {
                let mut buf = Buffer::empty(Rect::new(0, 0, 8, 9));
                let markers = ordered
                    .into_iter()
                    .map(|(index, loved, playing)| MinimapEntry {
                        index,
                        loved,
                        playing,
                    });
                render_pinned(&mut buf, track, 101, cursor, markers, &theme);
                buf
            };
            // 无光标：只有菱形与红轨道，右列没有任何点。
            let plain = render(None);
            for (y, symbol) in (track.top()..track.bottom()).zip("◆◆⢸⢸⢸".chars()) {
                assert_eq!(cell(&plain, track.x, y)?.symbol(), symbol.to_string());
            }
            for y in [2, 3] {
                assert_eq!(cell(&plain, 4, y)?.fg, theme.green);
            }
            for y in [4, 5, 6] {
                assert_eq!(cell(&plain, 4, y)?.fg, theme.red);
            }
            // 末项（已喜欢）与光标同格时右列归光标，喜欢红让位；
            // 光标落在轨道末端、离在播点够远，不会被吸附区抢走。
            let shared = render(Some(POSITION_SCALE));
            let cursor_cell = cell(&shared, 4, 6)?;
            assert!(
                matches!(cursor_cell.symbol(), "⢹" | "⢺" | "⢼" | "⣸"),
                "压缩格里的光标仍占右列一个点"
            );
            assert_ne!(cursor_cell.fg, theme.red, "光标格不再保留喜欢红");
            assert_eq!(
                cell(&shared, 4, 4)?.symbol(),
                "⢸",
                "光标不在的喜欢格仍是纯轨道"
            );
        }
        Ok(())
    }

    /// 单格内四个点位依次可见，末点之后接下一格的首点，覆盖整个轨道。
    #[test]
    fn cursor_uses_four_dot_rows_and_crosses_cell_edges() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        for (height, position, row, symbol) in [
            (1, 0, 0, "⢹"),
            (1, 333_333, 0, "⢺"),
            (1, 666_667, 0, "⢼"),
            (1, POSITION_SCALE, 0, "⣸"),
            (2, 0, 0, "⢹"),
            (2, 142_857, 0, "⢺"),
            (2, 285_714, 0, "⢼"),
            (2, 428_571, 0, "⣸"),
            (2, 571_429, 1, "⢹"),
            (2, 714_286, 1, "⢺"),
            (2, 857_143, 1, "⢼"),
            (2, POSITION_SCALE, 1, "⣸"),
        ] {
            let track = Rect::new(0, 0, 1, height);
            let mut buf = Buffer::empty(track);
            render_pinned(
                &mut buf,
                track,
                101,
                Some(position),
                std::iter::empty(),
                &theme,
            );
            assert_eq!(cell(&buf, 0, row)?.symbol(), symbol);
            assert_eq!(
                buf.content
                    .iter()
                    .filter(|cell| matches!(cell.symbol(), "⢹" | "⢺" | "⢼" | "⣸"))
                    .count(),
                1,
                "轨道内只显示一个光标中心"
            );
        }
        Ok(())
    }

    /// 短列表里连着的喜欢在轨道上连成一片：红色按各项的等分区间铺满，隔一项则留空。
    #[test]
    fn adjacent_loved_tracks_tint_one_continuous_run() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        let track = Rect::new(0, 0, 1, 9);
        let render = |loved: &[usize]| {
            let mut buf = Buffer::empty(track);
            render_pinned(
                &mut buf,
                track,
                3,
                None,
                loved.iter().map(|&index| MinimapEntry {
                    index,
                    loved: true,
                    playing: false,
                }),
                &theme,
            );
            buf
        };
        // 第 0、1 项连着：两项的区间在盲文点 9 处相接，红色从轨道首行连到第 6 行。
        let run = render(&[0, 1]);
        for y in 0..=6 {
            assert_eq!(cell(&run, 0, y)?.fg, theme.red, "第 {y} 行属于连着的喜欢段");
        }
        for y in 7..9 {
            assert_eq!(cell(&run, 0, y)?.fg, theme.surface1, "第 {y} 行在段外");
        }
        // 第 0、2 项之间隔着第 1 项：各自红自己那一份，中间三行仍是轨道色。
        let apart = render(&[0, 2]);
        for y in 0..=2 {
            assert_eq!(cell(&apart, 0, y)?.fg, theme.red, "第 {y} 行属于第 0 项");
        }
        for y in 3..=5 {
            assert_eq!(
                cell(&apart, 0, y)?.fg,
                theme.surface1,
                "第 {y} 行属于没喜欢的第 1 项"
            );
        }
        for y in 6..9 {
            assert_eq!(cell(&apart, 0, y)?.fg, theme.red, "第 {y} 行属于第 2 项");
        }
        Ok(())
    }

    /// 单项列表里唯一一项占满整条轨道；空列表没有喜欢可染。
    #[test]
    fn single_item_loved_run_covers_the_whole_rail() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        let track = Rect::new(0, 0, 1, 9);
        let mut buf = Buffer::empty(track);
        render_pinned(
            &mut buf,
            track,
            1,
            None,
            std::iter::once(MinimapEntry {
                index: 0,
                loved: true,
                playing: false,
            }),
            &theme,
        );
        for y in track.top()..track.bottom() {
            assert_eq!(cell(&buf, 0, y)?.fg, theme.red, "第 {y} 行属于唯一一项");
        }
        Ok(())
    }

    /// 同格压进多首喜欢曲目只染红轨道；光标进同一格时把那格让给光标点。
    #[test]
    fn loved_tracks_tint_the_rail_and_leave_the_cursor_column_clear() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        let track = Rect::new(0, 0, 1, 1);
        for (cursor, symbol, tinted_by_cursor) in [
            (None, "⢸", false),
            (Some(0), "⢹", true),
            (Some(POSITION_SCALE / 3), "⢺", true),
            (Some(POSITION_SCALE * 2 / 3), "⢼", true),
        ] {
            let mut buf = Buffer::empty(track);
            render_pinned(
                &mut buf,
                track,
                4,
                cursor,
                [0, 2].into_iter().map(|index| MinimapEntry {
                    index,
                    loved: true,
                    playing: false,
                }),
                &theme,
            );
            assert_eq!(cell(&buf, 0, 0)?.symbol(), symbol);
            if tinted_by_cursor {
                assert_ne!(cell(&buf, 0, 0)?.fg, theme.red, "光标格不再保留喜欢红");
            } else {
                assert_eq!(cell(&buf, 0, 0)?.fg, theme.red);
            }
        }
        Ok(())
    }

    /// 吸附区是在播点上下各两个点：区内光标点不显示、光晕全灭，在播格取光标色。
    #[test]
    fn magnet_absorbs_the_cursor_within_two_dots() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        let track = Rect::new(0, 0, 1, 9);
        // 第 5 项投到第 4 行（盲文点 18），吸附区是点 16..20。
        let render = |dot: u32| {
            let mut buf = Buffer::empty(track);
            render_pinned(
                &mut buf,
                track,
                9,
                Some(dot * POSITION_SCALE / 35),
                std::iter::once(MinimapEntry {
                    index: 4,
                    loved: false,
                    playing: true,
                }),
                &theme,
            );
            buf
        };
        // 区内（含两侧边缘）：光标点不落笔，只剩轨道和在播格。
        for (dot, row) in [(16_u32, 4_u16), (18, 4), (20, 5)] {
            let buf = render(dot);
            assert_eq!(cell(&buf, 0, 4)?.symbol(), "◆", "点 {dot} 吸住");
            assert_eq!(cell(&buf, 0, 4)?.fg, theme.accent, "被吸住则取光标色");
            if row != 4 {
                assert_eq!(cell(&buf, 0, row)?.symbol(), "⢸", "点 {dot} 不在区内留点");
            }
        }
        assert_eq!(cell(&render(16), 0, 3)?.fg, theme.surface1, "吸住后无光晕");
        assert_eq!(cell(&render(18), 0, 3)?.fg, theme.surface1, "吸住后无光晕");

        // 区外（点 15 / 21 在上下第三个点）：光标留在自己的格，光晕照常，在播格主题绿。
        for (dot, row, symbol) in [(15_u32, 3_u16, "⣸"), (21, 5, "⢺")] {
            let buf = render(dot);
            assert_eq!(
                cell(&buf, 0, row)?.symbol(),
                symbol,
                "点 {dot} 留在自己的格"
            );
            assert_ne!(cell(&buf, 0, 3)?.fg, theme.surface1, "区外光晕照常");
            assert_eq!(cell(&buf, 0, 4)?.fg, theme.green, "在播格不吃光标渐变");
        }
        Ok(())
    }

    /// 密集列表的光晕受最小半径控制，吸附半径决定吸走范围。
    #[test]
    fn minimap_geometry_follows_configuration() -> color_eyre::Result<()> {
        let mut theme = default_theme()?;
        theme.surface1 = Color::Rgb(0, 0, 0);
        theme.accent = Color::Rgb(240, 240, 240);
        let track = Rect::new(0, 0, 1, 9);
        // 在播点固定在 18 行；光标停在 28（区外、光晕可见）或 20（吸附边缘）。
        let render = |halo_rows: u64, magnet_dots: u64, dot: u32| {
            let mut buf = Buffer::empty(track);
            render_minimap(
                &mut buf,
                track,
                9,
                MinimapCursor {
                    position: Some(dot * POSITION_SCALE / 35),
                    magnet: &MagnetProgress::default(),
                    ticks: 1,
                    advancing: true,
                    halo_rows,
                    magnet_dots,
                },
                std::iter::once(MinimapEntry {
                    index: 4,
                    loved: false,
                    playing: true,
                }),
                &theme,
            );
            buf
        };
        let tight = render(1, MAGNET_DOTS, 28);
        assert_eq!(cell(&tight, 0, 3)?.fg, theme.surface1, "半径 1 不外溢");
        let wide = render(5, MAGNET_DOTS, 28);
        assert_ne!(cell(&wide, 0, 3)?.fg, theme.surface1, "半径 5 点到第 3 行");
        let unabsorbed = render(HALO_ROWS, 0, 20);
        assert_eq!(cell(&unabsorbed, 0, 5)?.symbol(), "⢹", "半径 0 留下光标点");
        let absorbed = render(HALO_ROWS, 2, 20);
        assert_eq!(cell(&absorbed, 0, 5)?.symbol(), "⢸", "半径 2 吸走光标点");
        Ok(())
    }

    /// 喜欢标记与光晕共用 per-item 平分区间，随轨道高度和条目数变化；单项占满轨道。
    #[test]
    fn halo_and_loved_markers_share_per_item_partitions() -> color_eyre::Result<()> {
        let mut theme = default_theme()?;
        theme.surface1 = Color::Rgb(0, 0, 0);
        theme.accent = Color::Rgb(240, 240, 240);
        for (total, height) in [(3, 41), (3, 81), (9, 81), (1, 41)] {
            let track = Rect::new(0, 0, 1, height);
            for index in [0, total / 2, total - 1] {
                let mut loved = Buffer::empty(track);
                render_pinned(
                    &mut loved,
                    track,
                    total,
                    None,
                    std::iter::once(MinimapEntry {
                        index,
                        loved: true,
                        playing: false,
                    }),
                    &theme,
                );
                let mut halo = Buffer::empty(track);
                render_pinned(
                    &mut halo,
                    track,
                    total,
                    crate::runtime::scroll::position::relative_position(index, total),
                    std::iter::empty(),
                    &theme,
                );
                let partition_start = loved.content.iter().position(|cell| cell.fg == theme.red);
                let partition_end = loved.content.iter().rposition(|cell| cell.fg == theme.red);
                let cursor_row = halo.content.iter().position(|cell| cell.symbol() != "⢸");
                let (Some(start), Some(end), Some(center)) =
                    (partition_start, partition_end, cursor_row)
                else {
                    color_eyre::eyre::bail!("非空列表必须显示条目区间和光标");
                };
                // 渐变尾部可能被 RGB 量化为底色；用区间内半段验证范围随条目数和高度扩展。
                let inner_half = (start + center) / 2..=(end + center) / 2;
                for y in 0..height {
                    let fg = cell(&halo, 0, y)?.fg;
                    if cell(&loved, 0, y)?.fg != theme.red {
                        assert_eq!(fg, theme.surface1, "第 {y} 行在 per-item 区间外");
                    } else if inner_half.contains(&usize::from(y)) {
                        assert_ne!(
                            fg, theme.surface1,
                            "{height} 行轨道、{total} 项中的第 {index} 项：第 {y} 行在区间内半段"
                        );
                    }
                }
            }
        }
        Ok(())
    }

    /// 吸附进出是缓动而非瞬变：在播格颜色逐帧淡向光标色，光晕同步淡出。
    #[test]
    fn magnet_fades_the_playing_color_across_frames() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        let track = Rect::new(0, 0, 1, 9);
        let magnet = MagnetProgress::default();
        let render = |dot: u32| {
            let mut buf = Buffer::empty(track);
            render_minimap(
                &mut buf,
                track,
                9,
                MinimapCursor {
                    position: Some(dot * POSITION_SCALE / 35),
                    magnet: &magnet,
                    ticks: 8,
                    advancing: true,
                    halo_rows: HALO_ROWS,
                    magnet_dots: MAGNET_DOTS,
                },
                std::iter::once(MinimapEntry {
                    index: 4,
                    loved: false,
                    playing: true,
                }),
                &theme,
            );
            buf
        };
        // 先在区外把进度稳定在「没吸住」。
        for _ in 0..3 {
            render(28);
        }
        assert_eq!(cell(&render(28), 0, 4)?.fg, theme.green, "区外保持主题绿");
        // 光标进区：首帧是过渡色，逐帧淡向光标色。
        let first = cell(&render(18), 0, 4)?.fg;
        assert_ne!(first, theme.green, "第一帧就开始过渡");
        assert_ne!(first, theme.accent, "但不是一帧到位");
        let mut previous = first;
        for _ in 0..7 {
            let now = cell(&render(18), 0, 4)?.fg;
            assert_ne!(now, previous, "逐帧继续过渡");
            previous = now;
        }
        for _ in 0..8 {
            render(18);
        }
        assert_eq!(cell(&render(18), 0, 4)?.fg, theme.accent, "最终停在光标色");
        assert_eq!(cell(&render(18), 0, 3)?.fg, theme.surface1, "光晕同步淡出");
        // 离开吸附区：同样逐帧淡回主题绿。
        let leaving = cell(&render(28), 0, 4)?.fg;
        assert_ne!(leaving, theme.accent, "离开时也开始过渡");
        for _ in 0..10 {
            render(28);
        }
        assert_eq!(cell(&render(28), 0, 4)?.fg, theme.green, "最终淡回主题绿");
        Ok(())
    }

    /// 靠近格边的在播曲目与其光标仍落在同一格，不能分别使用整格和盲文点坐标。
    #[test]
    fn cursor_and_playing_marker_share_subcell_projection() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        let track = Rect::new(0, 0, 1, 3);
        for (index, row) in [(0, 0), (3, 0), (4, 1), (7, 1), (8, 2), (11, 2)] {
            let mut buf = Buffer::empty(track);
            render_pinned(
                &mut buf,
                track,
                12,
                crate::runtime::scroll::position::relative_position(index, 12),
                std::iter::once(MinimapEntry {
                    index,
                    loved: true,
                    playing: true,
                }),
                &theme,
            );
            assert_eq!(cell(&buf, 0, row)?.symbol(), "◆");
            assert!(
                buf.content
                    .iter()
                    .filter(|cell| cell.symbol() != "◆")
                    .all(|cell| cell.symbol() == "⢸")
            );
        }
        Ok(())
    }

    /// 空轨道不落笔，空列表只显示盲文轨道；单曲、双曲和单行轨道保持首尾定位。
    #[test]
    fn empty_and_short_lists_respect_track_endpoints() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        let original = Buffer::filled(Rect::new(0, 0, 8, 9), Cell::new("╭"));
        for track in [Rect::new(4, 2, 0, 5), Rect::new(4, 2, 1, 0)] {
            let mut buf = original.clone();
            render_pinned(
                &mut buf,
                track,
                1,
                Some(0),
                std::iter::once(MinimapEntry {
                    index: 0,
                    loved: true,
                    playing: true,
                }),
                &theme,
            );
            assert_eq!(buf, original, "没有绘制区域时保留原边框");
        }
        for (total, height, cursor, first, last) in [
            (0, 5, 0, "⢸", "⢸"),
            (1, 5, 0, "◆", "⢸"),
            (2, 5, 0, "⢹", "◆"),
            (2, 5, POSITION_SCALE, "⢸", "◆"),
            (2, 1, POSITION_SCALE, "◆", "◆"),
        ] {
            let track = Rect::new(4, 2, 1, height);
            let mut buf = original.clone();
            let entries = (0..total).map(|index| MinimapEntry {
                index,
                loved: true,
                playing: index + 1 == total,
            });
            render_pinned(&mut buf, track, total, Some(cursor), entries, &theme);
            assert_eq!(cell(&buf, track.x, track.top())?.symbol(), first);
            assert_eq!(cell(&buf, track.x, track.bottom() - 1)?.symbol(), last);
            assert_eq!(cell(&buf, track.x, track.top() - 1)?.symbol(), "╭");
            assert_eq!(cell(&buf, track.x, track.bottom())?.symbol(), "╭");
            if total == 0 {
                for y in track.top()..track.bottom() {
                    assert_eq!(cell(&buf, track.x, y)?.symbol(), "⢸");
                    assert_eq!(cell(&buf, track.x, y)?.fg, theme.surface1);
                }
            }
        }
        Ok(())
    }

    /// 十万曲目的后半段仍映射到正确行；表头、圆角及轨道左右的整格内容保持不变。
    #[test]
    fn large_list_positions_stay_inside_the_track() -> color_eyre::Result<()> {
        let theme = default_theme()?;
        let area = Rect::new(10, 20, 5, 13);
        let track = Rect::new(12, 22, 1, 9);
        let original = Buffer::filled(area, Cell::new("#"));
        let mut buf = original.clone();
        let total = 100_001;
        let entries = (0..=total).map(|index| MinimapEntry {
            index,
            loved: matches!(index, 0 | 25_000 | 75_000),
            playing: index == 100_000 || index == total,
        });
        render_pinned(
            &mut buf,
            track,
            total,
            Some(POSITION_SCALE / 2),
            entries,
            &theme,
        );
        for (y, symbol) in (track.top()..track.bottom()).zip("⢸⢸⢸⢸⢼⢸⢸⢸◆".chars())
        {
            assert_eq!(cell(&buf, track.x, y)?.symbol(), symbol.to_string());
        }
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                if !track.contains(Position::new(x, y)) {
                    assert_eq!(cell(&buf, x, y)?, cell(&original, x, y)?);
                }
            }
        }
        Ok(())
    }

    /// 光标在同一格内移动仍改变中心与两侧颜色，换主题立即改变轨道和全部标记色。
    #[test]
    fn fractional_cursor_and_theme_change_cell_colors() -> color_eyre::Result<()> {
        let mut theme = default_theme()?;
        theme.surface1 = Color::Rgb(0, 0, 0);
        theme.accent = Color::Rgb(240, 240, 240);
        let track = Rect::new(0, 0, 1, 15);
        let render = |cursor, theme: &crate::render::theme::Theme| {
            let mut buf = Buffer::empty(track);
            let entries = (0..15).map(|index| MinimapEntry {
                index,
                loved: matches!(index, 0 | 4),
                playing: index == 14,
            });
            render_pinned(&mut buf, track, 15, Some(cursor), entries, theme);
            buf
        };
        let centered = render(POSITION_SCALE / 2, &theme);
        let fractional = render(POSITION_SCALE * 17 / 32, &theme);
        assert_eq!(cell(&centered, 0, 7)?.fg, theme.accent, "居中的光标格满亮");
        for y in [3, 6] {
            assert_eq!(cell(&centered, 0, y)?.symbol(), "⢸");
            assert_ne!(cell(&centered, 0, y)?.fg, theme.surface1);
            assert_ne!(cell(&centered, 0, y)?.fg, theme.accent);
            assert_ne!(cell(&centered, 0, y)?.fg, cell(&fractional, 0, y)?.fg);
        }
        assert_eq!(cell(&centered, 0, 7)?.symbol(), "⢼");
        assert_eq!(cell(&fractional, 0, 7)?.symbol(), "⣸");
        assert_ne!(cell(&centered, 0, 7)?.fg, cell(&fractional, 0, 7)?.fg);
        let Color::Rgb(center_red, _, _) = cell(&fractional, 0, 7)?.fg else {
            color_eyre::eyre::bail!("真彩主题的光标应保留 RGB 颜色");
        };
        let Color::Rgb(accent_red, _, _) = theme.accent else {
            color_eyre::eyre::bail!("测试主题应使用真彩强调色");
        };
        assert!(
            center_red > 120 && center_red < accent_red,
            "中心明亮但未满亮，同一格内移动仍可见色差"
        );
        // 半径外的喜欢与在播格保持主题原色，不参与光晕。
        assert_eq!(cell(&fractional, 0, 0)?.symbol(), "⢸");
        assert_eq!(cell(&fractional, 0, 0)?.fg, theme.red);
        assert_eq!(cell(&fractional, 0, 14)?.symbol(), "◆");
        assert_eq!(cell(&fractional, 0, 14)?.fg, theme.green);

        let mut changed = theme;
        changed.surface1 = Color::Rgb(30, 40, 50);
        changed.accent = Color::Rgb(70, 130, 190);
        changed.red = Color::Rgb(240, 90, 170);
        changed.green = Color::Rgb(90, 230, 140);
        let rethemed = render(POSITION_SCALE * 17 / 32, &changed);
        for y in 0..track.height {
            assert_eq!(
                cell(&fractional, 0, y)?.symbol(),
                cell(&rethemed, 0, y)?.symbol()
            );
            assert_ne!(cell(&fractional, 0, y)?.fg, cell(&rethemed, 0, y)?.fg);
        }
        // 换主题后标记格直接跟随新 token。
        let rethemed_top = render(0, &changed);
        assert_eq!(cell(&rethemed_top, 0, 14)?.fg, changed.green);
        assert_eq!(cell(&rethemed_top, 0, 0)?.fg, changed.accent, "光标格满亮");
        Ok(())
    }

    /// 光晕向外二次衰减，先快后慢、上下对称，并在半径边缘恢复纯轨道色。
    #[test]
    fn glow_spreads_wider_with_strict_falloff() -> color_eyre::Result<()> {
        let mut theme = default_theme()?;
        theme.surface1 = Color::Rgb(0, 0, 0);
        theme.accent = Color::Rgb(240, 240, 240);
        let Color::Rgb(accent_red, _, _) = theme.accent else {
            color_eyre::eyre::bail!("测试主题应使用真彩强调色");
        };
        let track = Rect::new(0, 0, 1, 11);
        let mut buf = Buffer::empty(track);
        render_pinned(
            &mut buf,
            track,
            11,
            Some(POSITION_SCALE / 2),
            std::iter::empty(),
            &theme,
        );
        let brightness = |y: u16| -> color_eyre::Result<u8> {
            let Color::Rgb(red, _, _) = cell(&buf, 0, y)?.fg else {
                color_eyre::eyre::bail!("真彩主题的光晕应保留 RGB 颜色");
            };
            Ok(red)
        };
        assert_eq!(brightness(5)?, accent_red, "光标中心满亮");
        let mut previous = accent_red;
        let mut previous_drop = None;
        for distance in 1..=4 {
            let below = brightness(5 + distance)?;
            let above = brightness(5 - distance)?;
            assert_eq!(below, above, "居中光标的光晕应上下对称");
            assert!(below < previous, "中心外第 {distance} 行应更暗");
            let drop = previous - below;
            if let Some(previous_drop) = previous_drop {
                assert!(drop < previous_drop, "越靠近边缘，每行亮度下降越慢");
            }
            previous_drop = Some(drop);
            previous = below;
        }
        assert!(previous > 0, "光晕至少覆盖中心外四行");
        assert_eq!(brightness(0)?, 0, "半径外恢复纯轨道色");
        assert_eq!(brightness(10)?, 0, "半径外恢复纯轨道色");
        Ok(())
    }
}

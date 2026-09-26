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

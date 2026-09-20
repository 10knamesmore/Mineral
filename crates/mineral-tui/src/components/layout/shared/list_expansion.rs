//! 清除筛选的逐行展开：按实体移动旧行，在空出的间隙补入完整列表的行。

use std::ops::Range;

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::render::color::lerp_color;
use crate::render::theme::Theme;
use crate::runtime::state::{
    FilteredListFrame, ListExpansion, ListExpansionScope, ListExpansionState, ListRowIdentity,
};

/// 一行的亚字符格位置，千分之一行。
const ROW_SCALE: i64 = 1000;

/// 本拍竞争一个字符行的画面；同一行只取权重最高者，避免文字互相拼接。
#[derive(Clone, Copy)]
struct RowPaint {
    /// 清除前的可见行。
    old_row: Option<u16>,

    /// 完整列表的可见行。
    new_row: Option<u16>,

    /// 位置插值与新行淡入共同决定的亮度，千分比。
    weight: u16,

    /// 选中行额外携带高亮底色，边界钳制换行时随文字移动。
    selected: bool,

    /// 图片只归属于离连续位置最近的一个字符行。
    images: bool,
}

/// 列表绘制前的几何与背景；展开时用它擦除目标位置的提前高亮。
pub(crate) struct ListSurface {
    /// 所属列表。
    pub(crate) scope: ListExpansionScope,

    /// 含表头与底栏的面板范围。
    area: Rect,

    /// 数据行范围，布局由调用者决定。
    body: Rect,

    /// 动画期间保存实际氛围底色，不携带任何列表行。
    backdrop: Option<Buffer>,
}

/// 在 Table 覆盖背景前保留动画需要的底图；稳态不复制。
pub(crate) fn begin_list(
    buf: &Buffer,
    area: Rect,
    body: Rect,
    expansion: &ListExpansionState,
    scope: ListExpansionScope,
) -> ListSurface {
    let backdrop = expansion
        .active
        .as_ref()
        .filter(|active| active.before.scope == scope)
        .map(|_| copy_panel(buf, area));
    ListSurface {
        scope,
        area,
        body,
        backdrop,
    }
}

/// 完成列表渲染后缓存筛选帧，或把刚画好的完整列表合成为展开帧。
///
/// 调用者负责只在稳定布局中接入；body 允许有边框和无边框的表格复用同一合成。
pub(crate) fn finish_list(
    buf: &mut Buffer,
    expansion: &mut ListExpansionState,
    theme: &Theme,
    surface: ListSurface,
    visible: Range<usize>,
    identities: impl Iterator<Item = ListRowIdentity>,
    filtering: bool,
) {
    let ListSurface {
        scope,
        area,
        body,
        backdrop,
    } = surface;
    if filtering {
        expansion.filtered_frame = Some(FilteredListFrame {
            scope,
            body,
            buffer: copy_panel(buf, area),
            rows: identities.collect(),
        });
        return;
    }
    expansion.filtered_frame = None;
    let Some(active) = &expansion.active else {
        return;
    };
    if active.before.scope != scope
        || active.before.buffer.area != area
        || active.before.body != body
    {
        expansion.invalidate();
        return;
    }
    if let Some(backdrop) = backdrop {
        compose(buf, &backdrop, active, visible, theme);
    }
}

/// 只复制当前面板，缓存大小与视口有关，与歌单总长度无关。
fn copy_panel(buf: &Buffer, area: Rect) -> Buffer {
    let mut frame = Buffer::empty(area);
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if let Some(source) = buf.cell((x, y))
                && let Some(target) = frame.cell_mut((x, y))
            {
                *target = source.clone();
            }
        }
    }
    frame
}

/// 将原始索引换成可见窗口坐标；窗口外的终点停在邻接的一行外。
fn destination(index: usize, visible: &Range<usize>, height: u16) -> i64 {
    if index < visible.start {
        -ROW_SCALE
    } else {
        i64::from(
            u16::try_from(index - visible.start)
                .unwrap_or(height)
                .min(height),
        ) * ROW_SCALE
    }
}

/// 新出现的行从相邻命中项之间展开；边缘行从最近命中项外侧进入。
fn insertion_origin(
    index: usize,
    active: &ListExpansion,
    visible: &Range<usize>,
    height: u16,
) -> i64 {
    let before = active
        .rows
        .iter()
        .filter(|row| row.full_index < index)
        .max_by_key(|row| row.full_index);
    let after = active
        .rows
        .iter()
        .filter(|row| row.full_index > index)
        .min_by_key(|row| row.full_index);
    match (before, after) {
        (Some(before), Some(after)) => {
            let left = destination(before.full_index, visible, height);
            let right = destination(after.full_index, visible, height);
            let at = destination(index, visible, height);
            let from = i64::from(before.screen_row) * ROW_SCALE;
            let to = i64::from(after.screen_row) * ROW_SCALE;
            from + (to - from) * (at - left) / (right - left)
        }
        (Some(before), None) => (i64::from(before.screen_row) + 1) * ROW_SCALE,
        (None, Some(after)) => (i64::from(after.screen_row) - 1) * ROW_SCALE,
        // 零命中没有可保持的选中项，start 不会启动这类展开。
        (None, None) => destination(index, visible, height),
    }
}

/// 把一条连续位置拆到相邻字符行，并按距离提供亮度权重。
fn place_row(slots: &mut [Option<RowPaint>], position: i64, paint: RowPaint) {
    let lower = position.div_euclid(ROW_SCALE);
    let fraction = position.rem_euclid(ROW_SCALE);
    let image_row = if fraction > ROW_SCALE / 2 {
        lower + 1
    } else {
        lower
    };
    for (row, contribution) in [(lower, ROW_SCALE - fraction), (lower + 1, fraction)] {
        let images = paint.images && row == image_row;
        let Ok(row) = usize::try_from(row) else {
            continue;
        };
        let Some(slot) = slots.get_mut(row) else {
            continue;
        };
        let weight = i64::from(paint.weight) * contribution / ROW_SCALE;
        let Ok(weight) = u16::try_from(weight) else {
            continue;
        };
        if weight > 0 && slot.is_none_or(|current| weight > current.weight) {
            *slot = Some(RowPaint {
                weight,
                images,
                ..paint
            });
        }
    }
}

/// 原有行移动、新行淡入，最后覆盖选中行，保证它不会被交叉路径遮住。
fn compose(
    buf: &mut Buffer,
    backdrop: &Buffer,
    active: &ListExpansion,
    visible: Range<usize>,
    theme: &Theme,
) {
    let area = backdrop.area;
    let current = copy_panel(buf, area);
    let body = active.before.body;
    let progress = active.progress.eased_in_out();
    let p = i64::from(progress);
    let mut slots = vec![None; usize::from(body.height)];
    let mut selected_slots = vec![None; usize::from(body.height)];
    for row in &active.rows {
        let from = i64::from(row.screen_row) * ROW_SCALE;
        let to = destination(row.full_index, &visible, body.height);
        let new_row = visible
            .contains(&row.full_index)
            .then(|| u16::try_from(row.full_index - visible.start).ok())
            .flatten();
        let paint = RowPaint {
            old_row: Some(row.screen_row),
            new_row,
            weight: 1000,
            selected: row.full_index == active.selected_index,
            images: true,
        };
        let destination_slots = if paint.selected {
            &mut selected_slots
        } else {
            &mut slots
        };
        place_row(destination_slots, from + (to - from) * p / ROW_SCALE, paint);
    }
    for (index, screen_row) in visible.clone().zip(0_u16..) {
        if active.rows.iter().any(|row| row.full_index == index) {
            continue;
        }
        let from = insertion_origin(index, active, &visible, body.height);
        let to = i64::from(screen_row) * ROW_SCALE;
        place_row(
            &mut slots,
            from + (to - from) * p / ROW_SCALE,
            RowPaint {
                old_row: None,
                new_row: Some(screen_row),
                weight: progress,
                selected: false,
                images: true,
            },
        );
    }
    // 表头的 match 列、查询徽标与计数随同淡换；几何与边框始终固定。
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if body.contains((x, y).into()) {
                continue;
            }
            if let Some(target) = buf.cell_mut((x, y)) {
                let (cell, weight) = row_cell(
                    active.before.buffer.cell((x, y)),
                    current.cell((x, y)),
                    progress,
                );
                if let Some(cell) = cell {
                    paint_cell(target, &cell, weight, theme, /*images*/ true);
                }
            }
        }
    }
    for (slot, y) in slots.iter().zip(body.top()..body.bottom()) {
        for x in body.left()..body.right() {
            if let Some(target) = buf.cell_mut((x, y)) {
                if let Some(background) = backdrop.cell((x, y)) {
                    *target = background.clone();
                }
                if let Some(paint) = slot {
                    paint_row_cell(
                        target,
                        (x, body.y),
                        *paint,
                        active,
                        &current,
                        progress,
                        theme,
                    );
                }
            }
        }
    }
    for (slot, y) in selected_slots.iter().zip(body.top()..body.bottom()) {
        let Some(paint) = slot else {
            continue;
        };
        for x in body.left()..body.right() {
            if let Some(target) = buf.cell_mut((x, y)) {
                if let Some(background) = backdrop.cell((x, y)) {
                    *target = background.clone();
                }
                paint_row_cell(
                    target,
                    (x, body.y),
                    *paint,
                    active,
                    &current,
                    progress,
                    theme,
                );
            }
        }
    }
}

/// 从一行的新旧布局取字形，避免 match 列消失时内容直接跳位。
fn paint_row_cell(
    target: &mut Cell,
    origin: (u16, u16),
    paint: RowPaint,
    active: &ListExpansion,
    current: &Buffer,
    progress: u16,
    theme: &Theme,
) {
    let (x, y) = origin;
    let old = paint
        .old_row
        .and_then(|row| active.before.buffer.cell((x, y + row)));
    let new = paint.new_row.and_then(|row| current.cell((x, y + row)));
    let (cell, content_weight) = row_cell(old, new, progress);
    if paint.selected {
        target.bg = lerp_color(target.bg, theme.surface0, u64::from(paint.weight), 1000);
    }
    if let Some(cell) = cell {
        let weight = u32::from(paint.weight) * u32::from(content_weight) / 1000;
        if let Ok(weight) = u16::try_from(weight) {
            paint_cell(target, &cell, weight, theme, paint.images);
        }
    }
}

/// 相同字形保留，新旧字形先退后入；只有一个端点时由位置权重控制显隐。
fn row_cell(old: Option<&Cell>, new: Option<&Cell>, progress: u16) -> (Option<Cell>, u16) {
    match (old, new) {
        (Some(old), Some(new)) if old.symbol() != new.symbol() => {
            if progress < 500 {
                (Some(old.clone()), 1000 - progress * 2)
            } else {
                (Some(new.clone()), (progress - 500) * 2)
            }
        }
        (Some(old), Some(new)) => {
            let mut cell = if progress < 500 {
                old.clone()
            } else {
                new.clone()
            };
            if !cell.symbol().starts_with('\u{10EEEE}') {
                cell.fg = lerp_color(old.fg, new.fg, u64::from(progress), 1000);
                cell.underline_color = lerp_color(
                    old.underline_color,
                    new.underline_color,
                    u64::from(progress),
                    1000,
                );
            }
            (Some(cell), 1000)
        }
        (_, Some(new)) => (Some(new.clone()), 1000),
        (old, None) => (old.cloned(), 1000),
    }
}

/// 保留实际底色，用颜色渐变表现行间位置。Kitty 颜色编码图片 ID，必须原样搬运。
fn paint_cell(target: &mut Cell, source: &Cell, weight: u16, theme: &Theme, images: bool) {
    let background = target.bg;
    if source.symbol().starts_with('\u{10EEEE}') {
        // 图片归属在位置采样时确定，半格平局也只画一次；颜色携带协议编码。
        if images && weight >= 500 {
            *target = source.clone();
            target.bg = background;
        }
        return;
    }
    *target = source.clone();
    target.bg = background;
    let background = if background == Color::Reset {
        theme.base
    } else {
        background
    };
    target.fg = lerp_color(background, source.fg, u64::from(weight), 1000);
}

//! 计算图片在终端 cell 网格中的视觉正方区域。

use ratatui::layout::Rect;

/// 按真实 cell 像素比例在可用区域内计算视觉正方形。
///
/// # Params:
///   - `area`: 外部可用的 cell 区域
///   - `cell_pixels`: 单个终端 cell 的像素宽高
///
/// # Return:
///   居中后的视觉正方区域。
pub(crate) fn square_subarea(area: Rect, cell_pixels: (u16, u16)) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let cell_width = u32::from(cell_pixels.0).max(1);
    let cell_height = u32::from(cell_pixels.1).max(1);
    let full_width_height =
        u16::try_from(u32::from(area.width) * cell_width / cell_height).unwrap_or(area.height);
    if full_width_height <= area.height {
        return Rect::new(area.x, area.y, area.width, full_width_height.max(1));
    }
    let width = u16::try_from(u32::from(area.height) * cell_height / cell_width)
        .unwrap_or(area.width)
        .min(area.width)
        .max(1);
    let padding = area.width.saturating_sub(width) / 2;
    Rect::new(area.x.saturating_add(padding), area.y, width, area.height)
}

/// 在没有终端像素信息的离屏 cell buffer 中计算 1:2 cell 比例的视觉正方形。
///
/// # Params:
///   - `area`: 外部可用的 cell 区域
///
/// # Return:
///   居中后的视觉正方区域。
pub(crate) fn square_cells(area: Rect) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::new(area.x, area.y, 0, 0);
    }
    let width = area.width.min(area.height.saturating_mul(2));
    let height = width / 2;
    let padding = area.width.saturating_sub(width) / 2;
    Rect::new(area.x.saturating_add(padding), area.y, width, height)
}

//! 列表封面在 Table 完成整行高亮后覆盖图片列，并按滚动与离屏状态选择图片阶段。

use std::time::{Duration, Instant};

use mineral_model::MediaUrl;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};

use crate::image::{ImageEngine, ImageRenderPhase};
use crate::runtime::scroll::list::ScrollMotion;
use crate::runtime::state::AppState;

/// 封面列占两格字符宽,让等比方图接近文字高度;列间距仍由 Table 提供。
pub(crate) const THUMBNAIL_COLUMNS: u16 = 2;

/// 在已有表格的图片列逐行覆盖缓存封面；缺图时保留 Table 画好的空格与背景。
///
/// # Params:
///   - `buf`: 已完成 Table 渲染与整行高亮的目标缓冲
///   - `images`: 图片引擎；缩略图入口只消费已有预取与解码缓存，不由可见行发起下载
///   - `column`: 列求解器返回的图片列矩形，含单行表头，不含 block 边框
///   - `covers`: 与本帧可见范围顺序一致的封面；缺图项仍须传 `None`，避免后续行错位
pub(crate) fn render_table_thumbnails<'a>(
    buf: &mut Buffer,
    images: &ImageEngine,
    column: Rect,
    covers: impl IntoIterator<Item = Option<&'a MediaUrl>>,
) {
    if column.width != THUMBNAIL_COLUMNS {
        return;
    }
    // 所有封面列表均为单行表头、单行数据且无行边距；零高表格不会产生 overlay。
    let [_header, rows] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(column);
    for (y, cover) in (rows.top()..rows.bottom()).zip(covers) {
        images.render_thumbnail(cover, Rect::new(rows.x, y, rows.width, 1), buf);
    }
}

/// 准备与绘制使用同一封面列几何和可见行顺序。
pub(crate) fn prepare_table_thumbnails<'a>(
    images: &mut ImageEngine,
    column: Rect,
    covers: impl IntoIterator<Item = Option<&'a MediaUrl>>,
    phase: ImageRenderPhase,
) {
    if column.width != THUMBNAIL_COLUMNS {
        return;
    }
    let [_header, rows] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(column);
    for (y, cover) in (rows.top()..rows.bottom()).zip(covers) {
        images.prepare_thumbnail(cover, Rect::new(rows.x, y, rows.width, 1), phase);
    }
}

/// 保留可见行当前已就绪的图片身份，供转场旧端重绘。
pub(crate) fn snapshot_thumbnails<'a>(
    images: &ImageEngine,
    column: Rect,
    covers: impl IntoIterator<Item = Option<&'a MediaUrl>>,
) -> Vec<crate::image::InlineImage> {
    if column.width != THUMBNAIL_COLUMNS {
        return Vec::new();
    }
    let [_header, rows] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(column);
    (rows.top()..rows.bottom())
        .zip(covers)
        .filter_map(|(y, cover)| images.inline_image(cover?, Rect::new(rows.x, y, rows.width, 1)))
        .collect()
}

/// 选择列表封面阶段；离屏冻结优先于形变与选中变化防抖。
///
/// # Params:
///   - `state`: 现读全屏、页面过渡及 `cover.debounce_ms` 配置
///   - `motion`: 本帧列表是否在离屏合成或瞬态布局中冻结
///   - `last_sel_change`: 当前列表所属页面的选中变化时间，browse 与 detail 各自提供
pub(crate) fn thumbnail_phase(
    state: &AppState,
    motion: ScrollMotion,
    last_sel_change: Instant,
) -> ImageRenderPhase {
    if matches!(motion, ScrollMotion::Frozen) {
        ImageRenderPhase::Offscreen
    } else if !state.browse.fullscreen.settled() || !state.channel_search.active.settled() {
        ImageRenderPhase::Resizing
    } else if state.frame_now.saturating_duration_since(last_sel_change)
        < Duration::from_millis(*state.cfg.tui().cover().debounce_ms())
    {
        ImageRenderPhase::Scrolling
    } else {
        ImageRenderPhase::Stable
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_config::Config;
    use mineral_model::MediaUrl;
    use ratatui::buffer::Buffer;
    use ratatui::layout::{Constraint, Rect};
    use ratatui::widgets::{Row, StatefulWidget, Table, TableState};

    use super::{THUMBNAIL_COLUMNS, render_table_thumbnails};
    use crate::components::layout::shared::marquee::resolve_column_rects;
    use crate::image::ImageEngine;

    /// 与真实 Table 对照所有窄宽度:列宽不足时留空,够宽时只覆盖图片列的数据格。
    #[test]
    fn squeezed_thumbnail_columns_do_not_overwrite_neighbors() -> color_eyre::Result<()> {
        let mut images = ImageEngine::disabled_kitty(Arc::new(Config::defaults()?));
        let url = MediaUrl::remote("https://example.com/narrow-cover.png")?;
        images.insert_test_thumbnail(&url)?;
        let widths = [
            Constraint::Length(1),
            Constraint::Length(THUMBNAIL_COLUMNS),
            Constraint::Fill(1),
            Constraint::Length(6),
        ];
        for width in 0..45 {
            let area = Rect::new(8, 3, width, 4);
            let mut buf = Buffer::empty(area);
            StatefulWidget::render(
                Table::new([Row::new(["H", "X", "TITLE", "3:45"])], widths)
                    .header(Row::new(["", "", "title", "len"]))
                    .highlight_symbol("▌ "),
                area,
                &mut buf,
                &mut TableState::default().with_selected(Some(0)),
            );
            let columns = resolve_column_rects(area, &widths, 2);
            let column = columns
                .get(1)
                .copied()
                .ok_or_else(|| color_eyre::eyre::eyre!("缺少图片列"))?;
            let before = buf.clone();
            render_table_thumbnails(&mut buf, &images, column, [Some(&url)]);
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    if column.width == THUMBNAIL_COLUMNS
                        && (column.left()..column.right()).contains(&x)
                        && y == area.y + 1
                    {
                        assert!(
                            buf.cell((x, y))
                                .is_some_and(|c| c.symbol().contains('\u{10EEEE}'))
                        );
                    } else {
                        assert_eq!(
                            buf.cell((x, y)),
                            before.cell((x, y)),
                            "图片不能覆盖表头、间隔或邻列，width={width}"
                        );
                    }
                }
            }
        }
        Ok(())
    }
}

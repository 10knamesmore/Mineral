//! now_playing 面板主封面槽的单一源:内区纵切几何 + 当前主封面身份。
//! track / playlist 绘制与 page morph 封面飞行层共用,保证飞行端点与面板实画零漂移。

use mineral_model::MediaUrl;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, Borders};

use crate::runtime::state::{AppState, View};

/// now_playing 内区(去边框)纵切三段:上 cover / 中 2 行 KV / 底 1 行。内区画不下
/// (过窄 / 过矮)为 `None`,与面板绘制的早退同一阈值。
///
/// # Params:
///   - `area`: 面板整区(含边框)
///
/// # Return:
///   `[cover, kv, 底行]`;画不下为 `None`。
pub(crate) fn sections(area: Rect) -> Option<[Rect; 3]> {
    let inner = Block::new().borders(Borders::ALL).inner(area);
    if inner.height < 4 || inner.width < 8 {
        return None;
    }
    Some(
        Layout::vertical([
            Constraint::Min(0),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .areas(inner),
    )
}

/// 返回面板当前主封面的 URL。
pub(crate) fn url(state: &AppState) -> Option<MediaUrl> {
    match state.browse.view.current() {
        View::Playlists => {
            let playlist = state.selected_playlist()?;
            crate::image::collage::effective_cover_url(state, &playlist.data)
        }
        View::Library => {
            let tracks = state.filtered_tracks();
            let song = &tracks.get(state.browse.nav.track.sel())?.data.song;
            song.cover_url.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::sections;

    /// 纵切几何:cover 贴内区顶、KV 恒 2 行、底行恒 1 行,三段铺满内区高。
    #[test]
    fn sections_split_covers_inner() -> color_eyre::Result<()> {
        let area = Rect::new(0, 0, 40, 20);
        let [cover, kv, strip] =
            sections(area).ok_or_else(|| color_eyre::eyre::eyre!("常规尺寸应可切分"))?;
        assert_eq!(cover.y, 1, "cover 贴内区顶(边框内)");
        assert_eq!(kv.height, 2, "KV 区恒 2 行");
        assert_eq!(strip.height, 1, "底行恒 1 行");
        assert_eq!(
            cover.height + kv.height + strip.height,
            18,
            "三段铺满内区高(20 - 上下边框)"
        );
        Ok(())
    }

    /// 内区过小(与面板绘制早退同阈值)不切分。
    #[test]
    fn sections_reject_tiny_area() {
        assert!(sections(Rect::new(0, 0, 9, 5)).is_none(), "过矮应拒绝");
        assert!(sections(Rect::new(0, 0, 6, 20)).is_none(), "过窄应拒绝");
    }
}

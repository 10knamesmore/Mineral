//! 队列光标附近的封面候选,交给运行时统一预取并复用图片缓存。

use mineral_model::{MediaUrl, SourceKind};

use super::QueueOverlay;
use crate::runtime::state::AppState;

impl QueueOverlay {
    /// 按过滤后的光标与当前列表半径选取封面,选中项优先向两侧扩展。
    pub(crate) fn cover_candidates(&self, ctx: &AppState) -> Vec<(SourceKind, MediaUrl)> {
        if !ctx.images.supports_thumbnails() {
            return Vec::new();
        }
        let visible = self.visible(ctx);
        let selected = self.list.sel();
        let radius = *ctx.cfg.tui().prefetch().radius();
        let mut covers = Vec::new();
        let mut consider = |index: usize| {
            if let Some(song) = visible
                .get(index)
                .and_then(|&raw| ctx.player.queue.get(raw))
                && let Some(url) = song.cover_url.as_ref()
            {
                covers.push((song.source(), url.clone()));
            }
        };
        consider(selected);
        for distance in 1..=radius {
            if let Some(index) = selected.checked_sub(distance) {
                consider(index);
            }
            consider(selected.saturating_add(distance));
        }
        covers
    }
}

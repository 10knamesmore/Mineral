//! 下载列表的只读业务输入。

/// 下载明细与汇总由应用借入，光标和标题动画归组件持有。
#[derive(Clone, Copy)]
pub(crate) struct DownloadInput<'a> {
    /// 后端确认的下载明细。
    pub(crate) downloads: &'a [mineral_protocol::SongDownloadView],

    /// 下载汇总。
    pub(crate) downloads_summary: &'a mineral_protocol::DownloadSummary,

    /// 当前有效配置。
    pub(crate) cfg: &'a mineral_config::Config,

    /// 本次显式采样时间。
    pub(crate) frame_now: std::time::Instant,
}

impl DownloadInput<'_> {
    /// 列表光标与视口边缘的行距。
    pub(super) fn scrolloff(&self) -> usize {
        usize::from(*self.cfg.tui().behavior().scrolloff())
    }

    /// 列表视口移动拍数。
    pub(super) fn list_glide_ticks(&self) -> u16 {
        let anim = self.cfg.tui().animation();
        crate::render::anim::ticks16_from_ms(*anim.list_scroll_ms(), *anim.frame_tick_ms())
    }

    /// 位置标记移动拍数。
    pub(super) fn minimap_cursor_ticks(&self) -> u16 {
        let anim = self.cfg.tui().animation();
        crate::render::anim::ticks16_from_ms(*anim.minimap_cursor_ms(), *anim.frame_tick_ms())
    }
}

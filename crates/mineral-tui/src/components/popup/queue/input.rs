//! 队列组件借用的播放镜像、装饰与显示环境。

use crate::image::ReadyImages;
use mineral_model::{Song, SongId, SourceKind};
use ratatui::layout::Rect;
use rustc_hash::{FxHashMap, FxHashSet};
use std::time::Instant;

/// 队列组件所需的数据，既不持有模型也不能提交任务。
#[derive(Clone, Copy)]
pub(crate) struct QueueInput<'a> {
    /// 后端确认的队列顺序，重复歌曲保留各自位置。
    pub(crate) queue: &'a [Song],

    /// 当前播放在队列中的准确位置，悬空时为空。
    pub(crate) current: Option<usize>,

    /// 当前播放是否仍附着于队列。
    pub(crate) attached: bool,

    /// 已播放位置，单位毫秒。
    pub(crate) position_ms: u64,

    /// 当前是否正在播放。
    pub(crate) playing: bool,

    /// 各来源已经同步的收藏身份。
    pub(crate) liked: &'a FxHashMap<SourceKind, FxHashSet<SongId>>,

    /// 当前有效配置，输入和显示均现读。
    pub(crate) cfg: &'a mineral_config::Config,

    /// 预计播放结束时间所用的本地钟点。
    pub(crate) now: chrono::DateTime<chrono::Local>,

    /// 准备阶段显式采样的时刻。
    pub(crate) frame_now: Instant,

    /// 完整抽屉矩形，锚点计算与显示共用。
    pub(crate) panel_area: Rect,

    /// 已就绪图片的只读能力。
    pub(crate) images: ReadyImages<'a>,
}

impl QueueInput<'_> {
    /// 是否已经收藏该来源的歌曲。
    pub(crate) fn is_liked(&self, song: &Song) -> bool {
        self.liked
            .get(&song.source())
            .is_some_and(|ids| ids.contains(&song.id))
    }

    /// 列表与视口边缘保留的行距。
    pub(super) fn scrolloff(&self) -> usize {
        usize::from(*self.cfg.tui().behavior().scrolloff())
    }

    /// 当前配置的视口移动时长。
    pub(super) fn list_glide_ticks(&self) -> u16 {
        let anim = self.cfg.tui().animation();
        crate::render::anim::ticks16_from_ms(*anim.list_scroll_ms(), *anim.frame_tick_ms())
    }

    /// 当前配置的位置标记移动时长。
    pub(super) fn minimap_cursor_ticks(&self) -> u16 {
        let anim = self.cfg.tui().animation();
        crate::render::anim::ticks16_from_ms(*anim.minimap_cursor_ms(), *anim.frame_tick_ms())
    }
}

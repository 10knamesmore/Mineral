//! 曲目列表借用的数据与只读显示输入。

use super::TrackList;
use crate::components::frame::FrameEnv;
use crate::image::ReadyImages;
use crate::runtime::state::{FilteredTracks, PlaylistTracks};
use crate::runtime::view_model::PlaylistView;
use mineral_model::SongId;

/// 一个已打开歌单的共享数据；列表不拥有模型或后端连接。
#[derive(Clone, Copy)]
pub(crate) struct TrackInput<'a> {
    /// 当前歌单，尚未进入时为空。
    pub(crate) playlist: Option<&'a PlaylistView>,

    /// 已加载曲目和续页状态，缺失表示仍在等待首批数据。
    pub(crate) tracks: Option<&'a PlaylistTracks>,

    /// 曲目内容版本，用于复用过滤索引。
    pub(crate) generation: u64,

    /// 正在播放的歌曲身份。
    pub(crate) playing: Option<&'a SongId>,
}

/// 已准备组件的只读借用，绘制不获得全局状态或资源调度能力。
pub(crate) struct TrackView<'a> {
    /// 曲目列表自己的状态。
    pub(super) list: &'a TrackList,

    /// 本次共享数据。
    pub(super) input: TrackInput<'a>,

    /// 当前配置、主题与采样时间。
    pub(super) frame: FrameEnv<'a>,

    /// 只读图片资源。
    pub(super) images: ReadyImages<'a>,
}

impl TrackList {
    /// 借用模型与已准备状态组成显示输入，不复制曲目。
    pub(crate) fn view<'a>(
        &'a self,
        input: TrackInput<'a>,
        frame: FrameEnv<'a>,
        images: ReadyImages<'a>,
    ) -> TrackView<'a> {
        TrackView {
            list: self,
            input,
            frame,
            images,
        }
    }

    /// 读取本次数据对应的过滤顺序，缓存只保存索引。
    pub(crate) fn rows<'a>(&self, input: TrackInput<'a>) -> FilteredTracks<'a> {
        match (input.playlist, input.tracks) {
            (Some(playlist), Some(tracks)) => {
                self.filtered(&playlist.data.id, input.generation, &tracks.entries)
            }
            _ => FilteredTracks::empty(),
        }
    }
}

impl crate::components::lifecycle::PaintView for TrackView<'_> {
    /// 列表实例声明可见数据、位置、搜索和图片输入；共享曲库不复制进缓存。
    fn dependencies(
        &self,
        area: ratatui::layout::Rect,
        _env: FrameEnv<'_>,
        inputs: &mut crate::render::memo::Dependencies<'_>,
    ) {
        inputs.optional(self.input.playlist);
        inputs.observe(&self.input.generation);
        inputs.optional(self.input.playing);
        inputs.observe(
            &self
                .input
                .tracks
                .map(|tracks| (tracks.complete, tracks.next_offset)),
        );
        self.list.search.dependencies(inputs);
        let tracks = self.rows();
        let total = tracks.len();
        let viewport = usize::from(area.height.saturating_sub(3));
        inputs.observe(&total);
        self.list.scroll.dependencies(inputs, total, viewport);
        self.list
            .title
            .dependencies(inputs, self.frame.config.animation(), self.frame.now);
        inputs.observe(&self.list.stable);
        inputs.observe(&self.list.expansion.active.is_some());
        if self.list.stable && self.list.expansion.active.is_some() {
            inputs.time(self.frame.now);
        }
        for entry in tracks.iter() {
            inputs.observe(&(entry.data.index, entry.loved));
        }
        let offset = self.list.scroll.offset(total, viewport);
        let mut covers = Vec::new();
        for index in offset..offset.saturating_add(viewport).min(total) {
            if let Some(entry) = tracks.get(index) {
                inputs.observe(entry);
                covers.push(entry.data.song.cover_url.clone());
            }
        }
        self.images.dependencies(inputs, covers);
    }

    fn paint(
        &self,
        frame: &mut ratatui::Frame<'_>,
        area: ratatui::layout::Rect,
        _env: FrameEnv<'_>,
    ) {
        self.paint(area, frame.buffer_mut());
    }
}

impl TrackView<'_> {
    /// 按组件当前查询借出行，不暴露其可变交互状态。
    pub(super) fn rows(&self) -> FilteredTracks<'_> {
        self.list.rows(self.input)
    }
}

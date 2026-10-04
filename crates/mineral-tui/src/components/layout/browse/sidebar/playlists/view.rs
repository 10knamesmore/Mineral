//! 歌单列表的模型借用与只读显示输入。

use super::PlaylistList;
use crate::components::frame::FrameEnv;
use crate::image::ReadyImages;
use crate::runtime::state::LibraryData;

/// 歌单与深度搜索所需的共享模型，不包含播放器或任务调度器。
#[derive(Clone, Copy)]
pub(crate) struct PlaylistInput<'a> {
    /// 歌单和已加载曲目，供深度搜索及显示时长使用。
    pub(crate) library: &'a LibraryData,

    /// 是否正在等待后端数据，用于区分空列表与加载状态。
    pub(crate) loading: bool,
}

/// 歌单列表本次绘制的明确依赖。
pub(crate) struct PlaylistView<'a> {
    /// 列表自己的交互与过滤状态。
    pub(super) list: &'a PlaylistList,

    /// 借入的共享模型。
    pub(super) input: PlaylistInput<'a>,

    /// 本次配置、主题和时间。
    pub(super) frame: FrameEnv<'a>,

    /// 已就绪图片的只读入口。
    pub(super) images: ReadyImages<'a>,
}

impl PlaylistList {
    /// 组装只读视图，组件之外的数据始终借用。
    pub(crate) fn view<'a>(
        &'a self,
        input: PlaylistInput<'a>,
        frame: FrameEnv<'a>,
        images: ReadyImages<'a>,
    ) -> PlaylistView<'a> {
        PlaylistView {
            list: self,
            input,
            frame,
            images,
        }
    }
}

impl crate::components::lifecycle::PaintView for PlaylistView<'_> {
    /// 过滤与时长使用数据版本，图片只观察可见歌单。
    fn dependencies(
        &self,
        area: ratatui::layout::Rect,
        _env: FrameEnv<'_>,
        inputs: &mut crate::render::memo::Dependencies<'_>,
    ) {
        inputs.borrowed(self.input.library.playlists.as_slice());
        inputs.observe(&self.input.library.tracks_generation);
        inputs.observe(&self.input.loading);
        inputs.observe(&self.indexing_count());
        self.list.search.dependencies(inputs);
        let rows = self.rows();
        let total = rows.len();
        let viewport = usize::from(area.height.saturating_sub(3));
        self.list.scroll.dependencies(inputs, total, viewport);
        inputs.observe(&self.list.stable);
        inputs.observe(&self.list.expansion.active.is_some());
        if self.list.stable && self.list.expansion.active.is_some() {
            inputs.time(self.frame.now);
        }
        let offset = self.list.scroll.offset(total, viewport);
        let covers = rows.iter().skip(offset).take(viewport).map(|playlist| {
            crate::image::collage::effective_cover_url(
                self.input.library,
                self.images,
                &playlist.data,
            )
        });
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

impl PlaylistView<'_> {
    /// 当前显示顺序；只复制引用，不复制曲目或歌单。
    pub(super) fn rows(&self) -> Vec<&crate::runtime::view_model::PlaylistView> {
        self.list.rows(self.input.library, self.frame.config)
    }

    /// 深度搜索正在补齐的歌单数量。
    pub(super) fn indexing_count(&self) -> Option<usize> {
        let count = self.input.library.completing_playlists();
        (*self.frame.config.tui().search().deep().enabled() && count > 0).then_some(count)
    }

    /// 当前已经知道的歌单时长，未知曲目不加入合计。
    pub(super) fn duration_ms(&self, id: &mineral_model::PlaylistId) -> u64 {
        self.input.library.tracks.get(id).map_or(0, |tracks| {
            tracks
                .iter()
                .filter_map(|entry| entry.data.song.duration_ms)
                .sum()
        })
    }
}

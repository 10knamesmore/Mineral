//! 曲库列表的模型借用与只读显示输入。

use std::cell::OnceCell;

use super::CollectionList;
use super::window::CollectionWindow;
use crate::components::frame::FrameEnv;
use crate::image::ReadyImages;
use crate::runtime::collection::{CollectionId, CollectionRef};
use crate::runtime::state::LibraryData;
use mineral_model::MediaUrl;

/// 歌单与深度搜索所需的共享模型，不包含播放器或任务调度器。
#[derive(Clone, Copy)]
pub(crate) struct CollectionInput<'a> {
    /// 歌单和已加载曲目，供深度搜索及显示时长使用。
    pub(crate) library: &'a LibraryData,

    /// 是否正在等待后端数据，用于区分空列表与加载状态。
    pub(crate) loading: bool,
}

/// 曲库列表本次绘制的明确依赖。
pub(crate) struct CollectionView<'a> {
    /// 列表自己的交互与过滤状态。
    pub(super) list: &'a CollectionList,

    /// 借入的共享模型。
    pub(super) input: CollectionInput<'a>,

    /// 本次配置、主题和时间。
    pub(super) frame: FrameEnv<'a>,

    /// 已就绪图片的只读入口。
    pub(super) images: ReadyImages<'a>,

    /// 本次视图按需解析一次，依赖检查、表格与边框共用。
    /// 视图持有曲库的不可变借用，存续期间不需要使行引用失效。
    rows: OnceCell<Vec<CollectionRef<'a>>>,
}

impl CollectionList {
    /// 组装只读视图；未显示的列表不解析行，项目内容始终借自曲库。
    pub(crate) fn view<'a>(
        &'a self,
        input: CollectionInput<'a>,
        frame: FrameEnv<'a>,
        images: ReadyImages<'a>,
    ) -> CollectionView<'a> {
        CollectionView {
            list: self,
            input,
            frame,
            images,
            rows: OnceCell::new(),
        }
    }
}

impl crate::components::lifecycle::PaintView for CollectionView<'_> {
    /// 过滤与时长使用数据版本，图片只观察可见歌单。
    fn dependencies(
        &self,
        area: ratatui::layout::Rect,
        _env: FrameEnv<'_>,
        inputs: &mut crate::render::memo::Dependencies<'_>,
    ) {
        inputs.borrowed(self.input.library.playlists.as_slice());
        inputs.borrowed(self.input.library.albums.as_slice());
        inputs.observe(&(
            self.list.group(),
            self.list.drawer.selected,
            self.list.drawer.reveal.raw(),
            self.list.choosing_type(),
        ));
        let area = self.list.content_area(area);
        inputs.observe(&self.input.library.tracks_generation);
        inputs.observe(&self.input.loading);
        inputs.observe(&self.indexing_count());
        self.list.active().search.dependencies(inputs);
        let rows = self.rows();
        let total = rows.len();
        let viewport = CollectionWindow::scroll_height(area);
        self.list
            .active()
            .scroll
            .dependencies(inputs, total, viewport);
        inputs.observe(&self.list.active().stable);
        inputs.observe(&self.list.active().expansion.active.is_some());
        if self.list.active().stable && self.list.active().expansion.active.is_some() {
            inputs.time(self.frame.now);
        }
        let window = CollectionWindow::new(area, rows, &self.list.active().scroll);
        let covers = window
            .visible
            .filter_map(|index| rows.get(index))
            .map(|row| self.cover(row));
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

impl<'a> CollectionView<'a> {
    /// 准备阶段已有行引用时直接复用，不再解析身份。
    pub(super) fn with_rows(mut self, rows: Vec<CollectionRef<'a>>) -> Self {
        self.rows = OnceCell::from(rows);
        self
    }

    /// 歌单可用拼贴封面；专辑使用来源提供的封面。
    pub(super) fn cover(&self, row: &CollectionRef<'_>) -> Option<MediaUrl> {
        match *row {
            CollectionRef::Playlist(p) => {
                crate::image::collage::effective_cover_url(self.input.library, self.images, &p.data)
            }
            CollectionRef::Album(a) => a.data.cover_url.clone(),
        }
    }

    /// 首次读取时批量解析身份，此后返回同一份行引用。
    pub(super) fn rows(&self) -> &[CollectionRef<'a>] {
        self.rows.get_or_init(|| self.list.rows(self.input.library))
    }

    /// 深度搜索正在补齐的歌单与专辑数量。
    pub(super) fn indexing_count(&self) -> Option<usize> {
        let count = self.input.library.completing_collections();
        (*self.frame.config.search().deep().enabled() && count > 0).then_some(count)
    }

    /// 当前已经知道的歌单或专辑时长，未知曲目不加入合计。
    pub(super) fn duration_ms(&self, id: &CollectionId) -> u64 {
        id.tracks(self.input.library).map_or(0, |tracks| {
            tracks
                .iter()
                .filter_map(|entry| entry.data.song.duration_ms)
                .sum()
        })
    }
}

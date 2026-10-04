//! 清除本地搜索时保留上一份显示输入，并按行身份展开到完整列表。

use rustc_hash::FxHashMap;

use mineral_model::{CollectionIndex, PlaylistId};
use ratatui::layout::Rect;

use crate::render::anim::Transition;

use super::View;

/// 展开动画所属的列表，避免借用其他面板的旧画面。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ListExpansionScope {
    /// 浏览页的歌单或曲目列表。
    Browse(View),

    /// 当前队列浮层。
    Queue,
}

/// 列表行的身份；同一首歌在歌单中的不同位置分别参与动画。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ListRowIdentity {
    /// 外层歌单。
    Playlist(PlaylistId),

    /// 当前歌单中的一次曲目出现。
    Track(CollectionIndex),

    /// 当前队列快照中的具体位置；队列更新时整段动画失效。
    Queue(usize),
}

/// 最后准备的搜索结果，只保存一个可见窗口。
pub(crate) struct FilteredListFrame {
    /// 画面所属列表。
    pub(crate) scope: ListExpansionScope,

    /// 数据行范围，不含表头、边框或底栏。
    pub(crate) body: Rect,

    /// 可重新绘制的表格、图片和概览输入。
    pub(crate) content: crate::components::layout::shared::scroll_table::PreparedTable,

    /// 完整面板区域。
    pub(crate) area: Rect,

    /// 从第一条可见数据行开始的身份序列。
    pub(crate) rows: Vec<ListRowIdentity>,
}

/// 一条已显示的命中行在清除前后的坐标。
pub(crate) struct ExpandingRow {
    /// 在旧视口中的行号，不含边框和表头。
    pub(crate) screen_row: u16,

    /// 在完整列表中的下标。
    pub(crate) full_index: usize,
}

/// 一次清除动作的画面与进度；逻辑查询在启动前已经清空。
pub(crate) struct ListExpansion {
    /// 清除前的可见结果。
    pub(crate) before: FilteredListFrame,

    /// 可见命中行映射到完整列表的位置。
    pub(crate) rows: Vec<ExpandingRow>,

    /// 清除后仍选中的实体在完整列表中的下标。
    pub(crate) selected_index: usize,

    /// 从搜索结果到完整列表的进度。
    pub(crate) progress: Transition,
}

/// 搜索结果的最后一帧与正在播放的展开共用生命周期。
#[derive(Default)]
pub(crate) struct ListExpansionState {
    /// 搜索态准备时更新，清除时交给动画。
    pub(crate) filtered_frame: Option<FilteredListFrame>,

    /// 没有展开时不持有旧画面。
    pub(crate) active: Option<ListExpansion>,
}

impl ListExpansionState {
    /// 布局准备阶段替换筛选输入，并在几何变化时结束旧转场。
    pub(crate) fn prepare(&mut self, area: Rect, body: Rect, filtered: Option<FilteredListFrame>) {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.before.area != area || active.before.body != body)
        {
            self.invalidate();
        }
        self.filtered_frame = filtered;
    }

    /// 转场旧端的图片需求由生命周期保留，与是否执行绘制无关。
    pub(crate) fn retain_images(&mut self, images: &mut crate::image::ImageEngine) {
        if self.active.as_ref().is_some_and(|active| {
            active
                .before
                .content
                .images
                .iter()
                .any(|image| !images.inline_is_current(image))
        }) {
            self.invalidate();
            return;
        }
        if let Some(active) = &self.active {
            for image in &active.before.content.images {
                images.retain_inline(image);
            }
        }
    }

    /// 将可见行身份映射到完整列表，开始一次展开。
    pub(crate) fn start(
        &mut self,
        scope: ListExpansionScope,
        order: impl Iterator<Item = (ListRowIdentity, usize)>,
        selected_index: usize,
        ticks: u16,
    ) {
        let Some(before) = self
            .filtered_frame
            .take()
            .filter(|frame| frame.scope == scope)
        else {
            return;
        };
        let screen_rows = before
            .rows
            .iter()
            .cloned()
            .zip(0_u16..)
            .collect::<FxHashMap<_, _>>();
        let rows = order
            .filter_map(|(id, full_index)| {
                screen_rows.get(&id).map(|&screen_row| ExpandingRow {
                    screen_row,
                    full_index,
                })
            })
            .collect::<Vec<_>>();
        // 光标刚移动但尚未准备时没有可保持的选中行，直接显示当前逻辑位置。
        if !rows.iter().any(|row| row.full_index == selected_index) {
            return;
        }
        mineral_log::debug!(target: "tui", ?scope, visible_rows = rows.len(), selected_index, ticks, "start search result expansion");
        self.active = Some(ListExpansion {
            before,
            rows,
            selected_index,
            progress: Transition::expanding(ticks),
        });
    }

    /// 下一次输入立即接管逻辑列表，不等待画面过渡完成。
    pub(crate) fn interrupt(&mut self) {
        if self.active.take().is_some() {
            mineral_log::debug!(target: "tui", "interrupt search result expansion");
        }
    }

    /// 尺寸或数据顺序改变后丢弃旧坐标与快照。
    pub(crate) fn invalidate(&mut self) {
        self.interrupt();
        self.filtered_frame = None;
    }

    /// 与主循环同拍推进，完成后释放快照。
    pub(crate) fn tick(&mut self) {
        if let Some(active) = &mut self.active {
            active.progress.tick();
            if active.progress.settled() {
                self.active = None;
                mineral_log::debug!(target: "tui", "finish search result expansion");
            }
        }
    }

    /// 配置热更时保留在途动画相位，只更新速度。
    pub(crate) fn retempo(&mut self, ticks: u16) {
        if let Some(active) = &mut self.active {
            active.progress.retempo(ticks);
        }
    }
}

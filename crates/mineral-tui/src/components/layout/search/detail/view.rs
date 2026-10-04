//! 详情绘制只借用当前栈和歌曲装饰，不访问整个搜索页或应用。

use crate::components::frame::FrameEnv;
use crate::image::{ImageRenderPhase, ReadyImages};
use crate::runtime::state::DetailStack;
use mineral_model::{Song, SongId, SourceKind};
use rustc_hash::{FxHashMap, FxHashSet};

/// 详情正文共享的只读输入。
pub(crate) struct DetailPaint<'a> {
    /// 收藏身份集合。
    pub(crate) liked: &'a FxHashMap<SourceKind, FxHashSet<SongId>>,

    /// 详情列表获得焦点的程度。
    pub(crate) focus: u16,

    /// 本帧 loading 字形，已按显式动画计数选定。
    pub(crate) loading: &'a str,

    /// 当前配置、主题和时间。
    pub(crate) frame: FrameEnv<'a>,

    /// 已就绪图片。
    pub(crate) images: ReadyImages<'a>,

    /// 主图片区域的稳定程度。
    pub(crate) phase: ImageRenderPhase,
}

impl DetailPaint<'_> {
    /// 按歌曲来源查询收藏事实。
    pub(crate) fn is_liked(&self, song: &Song) -> bool {
        self.liked
            .get(&song.source())
            .is_some_and(|ids| ids.contains(&song.id))
    }
}

/// 外框和内容共同读取当前详情栈。
pub(crate) struct DetailView<'a> {
    /// 没有搜索结果时不存在详情栈。
    pub(crate) stack: Option<&'a DetailStack>,

    /// 正文绘制输入。
    pub(crate) paint: DetailPaint<'a>,
}

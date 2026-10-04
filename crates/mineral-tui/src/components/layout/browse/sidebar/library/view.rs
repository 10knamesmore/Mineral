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

impl TrackView<'_> {
    /// 按组件当前查询借出行，不暴露其可变交互状态。
    pub(super) fn rows(&self) -> FilteredTracks<'_> {
        self.list.rows(self.input)
    }
}

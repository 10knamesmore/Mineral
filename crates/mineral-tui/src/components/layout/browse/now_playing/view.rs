//! 选中项详情的显式输入；选择和模型查询由页面组合层完成。

use super::NowPlaying;
use crate::components::frame::FrameEnv;
use crate::image::{ImageRenderPhase, ReadyImages};
use crate::runtime::state::ViewSwitch;
use crate::runtime::view_model::{PlaylistEntryView, PlaylistView};
use mineral_model::{MediaUrl, SongId};

/// 浏览页当前选择与过渡两端的数据。
pub(crate) struct NowPlayingInput<'a> {
    /// 外层选中的歌单。
    pub(crate) playlist: Option<&'a PlaylistView>,

    /// 内层选中的曲目。
    pub(crate) track: Option<&'a PlaylistEntryView>,

    /// 当前播放歌曲身份。
    pub(crate) playing: Option<&'a SongId>,

    /// 歌单原图或已完成的拼贴身份。
    pub(crate) playlist_cover: Option<MediaUrl>,

    /// 可预热的入口曲封面。
    pub(crate) upcoming_cover: Option<MediaUrl>,

    /// 歌单已知曲目时长之和；未载入时缺省。
    pub(crate) duration_ms: Option<u64>,

    /// 父页面控制的两端过渡。
    pub(crate) switch: ViewSwitch,
}

/// 封面面板的完整只读绘制输入。
pub(crate) struct NowPlayingView<'a> {
    /// 面板自己的标题滚动状态。
    pub(crate) panel: &'a NowPlaying,

    /// 当前选择。
    pub(crate) input: NowPlayingInput<'a>,

    /// 当前主题、配置与时间。
    pub(crate) frame: FrameEnv<'a>,

    /// 已完成的图片资源。
    pub(crate) images: ReadyImages<'a>,

    /// 当前图片布局阶段。
    pub(crate) phase: ImageRenderPhase,
}

//! 选中项详情面板自己保留的显示状态。

/// 右栏选中项详情，随浏览页保留标题动画。
#[derive(Default)]
pub struct NowPlaying {
    /// 当前标题的滚动起点。
    pub(crate) title: crate::runtime::marquee::Marquee,
}

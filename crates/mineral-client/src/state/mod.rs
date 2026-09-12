//! 供客户端读取的本地状态镜像，以及消费式的事件和音频数据。
//!
//! 会话应用 daemon 的订阅更新，调用方通过 [`Mirror`] 读取状态。用
//! [`Mirror::subscription_seen`] 判断订阅首帧是否已到达；部分视图在未就绪时返回初始值。
//! 播放位置可根据最近锚点在本地推进。
//! 事件、PCM 样本和断续标记取走即清除，同一会话应集中消费后再分发给界面组件。

mod mirror;
mod pcm;

pub(crate) use mirror::ApplyOutcome;
pub use mirror::{
    DownloadsDetailMirror, Mirror, PlaybackMirror, PlayerMirror, WindowTitleOverride,
};

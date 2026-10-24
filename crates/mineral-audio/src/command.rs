//! AudioHandle 投递给 engine 线程的内部命令。

use mineral_common::revision::Revision;
use mineral_playback::OpenedMedia;
use std::time::Duration;

/// A seek belongs to the track visible when it was requested.
pub(crate) struct SeekRequest {
    pub(crate) track_token: Revision,

    pub(crate) position: Duration,
}

/// 投递给 engine 主循环的一条指令。
pub(crate) enum AudioCommand {
    /// 枚举 daemon 所在机器的输出设备，结果仅返回请求者。
    ListOutputs(tokio::sync::oneshot::Sender<crate::Result<Vec<crate::OutputDevice>>>),

    /// 切换输出设备；应答在新输出流启动成功或失败后发送。
    SelectOutput {
        /// 目标设备或系统默认策略。
        target: crate::OutputTarget,

        /// 切换结论。
        reply: tokio::sync::oneshot::Sender<crate::Result<()>>,
    },

    /// Replaces current playback with already-opened media.
    Play(OpenedMedia),

    /// Prepares the next track's PCM without interrupting current playback.
    ///
    /// Unlike [`Self::Play`], this command does not stop or resume the current decoder.
    AppendNext(OpenedMedia),

    /// Cancels the prefetched worker and releases its prepared PCM.
    ClearNext,

    /// 暂停当前曲目。
    Pause,

    /// 从暂停态恢复。
    Resume,

    /// 停掉当前曲目并取消解码与预排。
    Stop,

    /// 设置音量(0..=100)。
    SetVolume(u8),
}

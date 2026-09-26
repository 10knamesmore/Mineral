//! AudioHandle 投递给 engine 线程的内部命令。

use mineral_playback::OpenedMedia;

/// 投递给 engine 主循环的一条指令。
pub(crate) enum AudioCommand {
    /// 枚举 daemon 所在机器的输出设备，结果仅返回请求者。
    ListOutputs(tokio::sync::oneshot::Sender<color_eyre::Result<Vec<crate::OutputDevice>>>),

    /// 切换输出设备；应答在新输出流启动成功或失败后发送。
    SelectOutput {
        /// 目标设备或系统默认策略。
        target: crate::OutputTarget,

        /// 切换结论。
        reply: tokio::sync::oneshot::Sender<color_eyre::Result<()>>,
    },

    /// Replaces current playback with already-opened media.
    Play(OpenedMedia),

    /// Appends an already-opened decoder behind the current decoder for gapless playback.
    ///
    /// Unlike [`Self::Play`], this command does not stop or resume the current decoder.
    AppendNext(OpenedMedia),

    /// Cancels and disarms the decoder appended for gapless playback.
    ClearNext,

    /// 暂停当前曲目。
    Pause,

    /// 从暂停态恢复。
    Resume,

    /// 停掉当前曲目并清空 sink。
    Stop,

    /// 设置音量(0..=100)。
    SetVolume(u8),
    // seek 不走 channel,走 [`crate::handle::AudioHandle`] 的 `Arc<Mutex<Option<Duration>>>`
    // mailbox(latest-wins),engine 主循环每 tick `take()` 一次 —— 长按 ←/→ 时合并。
}

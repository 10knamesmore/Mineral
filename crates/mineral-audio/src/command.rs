//! AudioHandle 投递给 engine 线程的内部命令。

use mineral_playback::OpenedMedia;

/// 投递给 engine 主循环的一条指令。
pub(crate) enum AudioCommand {
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

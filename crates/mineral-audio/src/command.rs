//! UI → engine 的命令枚举。

use std::path::PathBuf;

use mineral_model::MediaUrl;

/// 投递给 engine 主循环的一条指令。
pub(crate) enum AudioCommand {
    /// 切到这个 URL,从头播。
    ///
    /// `capture` 非空时(仅对 `Remote`),边播边把下载的字节落到该路径,供播完后入缓存;
    /// `Local` 或 `capture` 为空时不落盘,维持原行为。
    Play {
        /// 播放源。
        url: MediaUrl,

        /// 捕获落盘路径(`Remote` + 想缓存时给)。
        capture: Option<PathBuf>,
    },
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

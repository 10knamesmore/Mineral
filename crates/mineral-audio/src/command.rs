//! UI → engine 的命令枚举。

use mineral_model::MediaUrl;

/// 投递给 engine 主循环的一条指令。
pub(crate) enum AudioCommand {
    /// 切到这个 URL,从头播。
    Play(MediaUrl),
    /// 暂停当前曲目。
    Pause,
    /// 从暂停态恢复。
    Resume,
    /// 停掉当前曲目并清空 sink。
    Stop,
    /// 跳到绝对位置(ms)。
    Seek(u64),
    /// 设置音量(0..=100)。
    SetVolume(u8),
}

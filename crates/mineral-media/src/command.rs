//! 来自系统媒体控件的控制命令(平台无关)。

use std::time::Duration;

/// 系统媒体控件(MPRIS / 键盘媒体键 / 桌面播放小组件)发来的控制命令。
///
/// 由后端把各平台的原生事件归一到本枚举,交给宿主(播放器)处理。只覆盖播放器
/// 能直接响应的子集;音量回写、Raise/Quit/OpenUri 等暂不纳入。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaCommand {
    /// 从暂停恢复播放。
    Play,

    /// 暂停。
    Pause,

    /// 播放 / 暂停切换。
    Toggle,

    /// 下一首。
    Next,

    /// 上一首。
    Previous,

    /// 停止。
    Stop,

    /// 相对当前位置快进给定时长。
    SeekForward(Duration),

    /// 相对当前位置快退给定时长。
    SeekBackward(Duration),

    /// 跳到绝对位置。
    SetPosition(Duration),

    /// 设置随机播放开关(MPRIS `Shuffle` 属性写入)。
    SetShuffle(bool),

    /// 设置循环模式(MPRIS `LoopStatus` 属性写入)。
    SetLoop(LoopMode),
}

/// 循环模式,对应 MPRIS `LoopStatus` 的三态;平台无关。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopMode {
    /// 不循环。
    None,

    /// 单曲循环。
    Track,

    /// 整列循环。
    Playlist,
}

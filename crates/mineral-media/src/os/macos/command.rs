//! 把系统媒体中心(`MPRemoteCommandCenter`)的命令接到平台无关的 [`MediaCommand`] 回调。
//!
//! 命令 block 由系统在**主线程 run loop** 上派发(无关本处注册所在线程),触发时调
//! `on_command`。本模块经 objc2 绑定调 Objective-C,`unsafe` 是 FFI 边界固有的。

#![allow(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use block2::RcBlock;
use core::ptr::NonNull;
use objc2_media_player::{
    MPChangePlaybackPositionCommandEvent, MPChangeRepeatModeCommandEvent,
    MPChangeShuffleModeCommandEvent, MPRemoteCommand, MPRemoteCommandCenter, MPRemoteCommandEvent,
    MPRemoteCommandHandlerStatus,
};

use super::convert::{repeat_to_loop, shuffle_to_bool};
use crate::command::MediaCommand;

/// 命令回调类型别名:收到系统媒体控件命令时调用,跨线程共享。
type OnCommand = Arc<dyn Fn(MediaCommand) + Send + Sync>;

/// 在共享命令中心上注册全部支持的命令,每条都接到 `on_command`。
///
/// 注册只做一次;命令中心内部持有 block,故 block 注册后即可释放本地引用。
pub(super) fn register_commands(on_command: &OnCommand) {
    let center = unsafe { MPRemoteCommandCenter::sharedCommandCenter() };

    wire_simple(
        &*unsafe { center.playCommand() },
        on_command,
        MediaCommand::Play,
    );
    wire_simple(
        &*unsafe { center.pauseCommand() },
        on_command,
        MediaCommand::Pause,
    );
    wire_simple(
        &*unsafe { center.stopCommand() },
        on_command,
        MediaCommand::Stop,
    );
    wire_simple(
        &*unsafe { center.togglePlayPauseCommand() },
        on_command,
        MediaCommand::Toggle,
    );
    wire_simple(
        &*unsafe { center.nextTrackCommand() },
        on_command,
        MediaCommand::Next,
    );
    wire_simple(
        &*unsafe { center.previousTrackCommand() },
        on_command,
        MediaCommand::Previous,
    );

    wire_position(
        &*unsafe { center.changePlaybackPositionCommand() },
        on_command,
    );
    wire_repeat(&*unsafe { center.changeRepeatModeCommand() }, on_command);
    wire_shuffle(&*unsafe { center.changeShuffleModeCommand() }, on_command);
}

/// 给一条命令挂上 handler block 并启用它。
fn attach(
    command: &MPRemoteCommand,
    handler: &RcBlock<dyn Fn(NonNull<MPRemoteCommandEvent>) -> MPRemoteCommandHandlerStatus>,
) {
    unsafe { command.setEnabled(true) };
    // 返回的 token 仅用于 removeTarget;我们注册一次永不移除,丢弃即可(block 由命令持有)。
    let _token = unsafe { command.addTargetWithHandler(handler) };
}

/// 接一个无参命令:每次触发都发同一条 [`MediaCommand`]。
fn wire_simple(command: &MPRemoteCommand, on_command: &OnCommand, cmd: MediaCommand) {
    let cb = Arc::clone(on_command);
    let handler = RcBlock::new(move |_event: NonNull<MPRemoteCommandEvent>| {
        cb(cmd);
        MPRemoteCommandHandlerStatus::Success
    });
    attach(command, &handler);
}

/// 接进度条拖动:读事件的 `positionTime`(秒)→ `SetPosition`。
fn wire_position(
    command: &objc2_media_player::MPChangePlaybackPositionCommand,
    on_command: &OnCommand,
) {
    let cb = Arc::clone(on_command);
    let handler = RcBlock::new(move |event: NonNull<MPRemoteCommandEvent>| {
        let pos = unsafe {
            event
                .cast::<MPChangePlaybackPositionCommandEvent>()
                .as_ref()
        };
        let secs = unsafe { pos.positionTime() }.max(0.0);
        cb(MediaCommand::SetPosition(Duration::from_secs_f64(secs)));
        MPRemoteCommandHandlerStatus::Success
    });
    attach(command.as_ref(), &handler);
}

/// 接循环模式切换:读事件的 `repeatType` → `SetLoop`。
fn wire_repeat(command: &objc2_media_player::MPChangeRepeatModeCommand, on_command: &OnCommand) {
    let cb = Arc::clone(on_command);
    let handler = RcBlock::new(move |event: NonNull<MPRemoteCommandEvent>| {
        let ev = unsafe { event.cast::<MPChangeRepeatModeCommandEvent>().as_ref() };
        let repeat = unsafe { ev.repeatType() };
        cb(MediaCommand::SetLoop(repeat_to_loop(repeat)));
        MPRemoteCommandHandlerStatus::Success
    });
    attach(command.as_ref(), &handler);
}

/// 接随机播放切换:读事件的 `shuffleType` → `SetShuffle`。
fn wire_shuffle(command: &objc2_media_player::MPChangeShuffleModeCommand, on_command: &OnCommand) {
    let cb = Arc::clone(on_command);
    let handler = RcBlock::new(move |event: NonNull<MPRemoteCommandEvent>| {
        let ev = unsafe { event.cast::<MPChangeShuffleModeCommandEvent>().as_ref() };
        let shuffle = unsafe { ev.shuffleType() };
        cb(MediaCommand::SetShuffle(shuffle_to_bool(shuffle)));
        MPRemoteCommandHandlerStatus::Success
    });
    attach(command.as_ref(), &handler);
}

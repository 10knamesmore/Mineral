//! 无系统媒体集成的平台占位:所有操作 no-op,使上层在该平台仍可编译运行。

use std::sync::Arc;
use std::time::Duration;

use crate::command::{LoopMode, MediaCommand};
use crate::config::MediaConfig;
use crate::state::{NowPlaying, PlaybackState};

/// 系统媒体服务句柄(无集成平台的占位)。
pub struct MediaService {
    /// 占位字段,使 stub 与真实实现保持「持有资源」的形态一致。
    _private: (),
}

impl MediaService {
    /// 占位:不接入任何系统集成,直接返回空句柄。
    ///
    /// # Params:
    ///   - `config`: 忽略。
    ///   - `on_command`: 忽略(本平台无命令来源)。
    pub fn spawn(
        config: &MediaConfig,
        on_command: Arc<dyn Fn(MediaCommand) + Send + Sync>,
    ) -> color_eyre::Result<Self> {
        let _ = config;
        let _ = on_command;
        Ok(Self { _private: () })
    }

    /// 占位:no-op。
    pub fn set_now_playing(&self, now_playing: &NowPlaying) -> color_eyre::Result<()> {
        let _ = now_playing;
        Ok(())
    }

    /// 占位:no-op。
    pub fn set_playback(
        &self,
        state: PlaybackState,
        position: Option<Duration>,
    ) -> color_eyre::Result<()> {
        let _ = (state, position);
        Ok(())
    }

    /// 占位:no-op。
    pub fn notify_seek(&self, position: Duration) -> color_eyre::Result<()> {
        let _ = position;
        Ok(())
    }

    /// 占位:no-op。
    pub fn set_shuffle(&self, shuffle: bool) -> color_eyre::Result<()> {
        let _ = shuffle;
        Ok(())
    }

    /// 占位:no-op。
    pub fn set_loop(&self, mode: LoopMode) -> color_eyre::Result<()> {
        let _ = mode;
        Ok(())
    }
}

//! 非 Linux 平台的 MediaService 占位:暂不接入系统媒体服务,所有操作 no-op。
//!
//! TODO(macos): 用 souvlaki 的 MPNowPlaying 后端实现,注意它要求在主线程跑
//! run loop 来 pump 事件,与无 GUI 的 daemon 进程需额外协调。

use std::sync::Arc;
use std::time::Duration;

use crate::command::MediaCommand;
use crate::config::MediaConfig;
use crate::state::{NowPlaying, PlaybackState};

/// 系统媒体服务句柄(非 Linux 占位)。
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
}

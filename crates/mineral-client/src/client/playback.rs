//! 播放传输与播放模式:提交控制请求并把 daemon 的 ack 结论翻成 [`Outcome`]。

use mineral_protocol::{PlayMode, Request};

use super::Client;
use crate::operation::{Outcome, decode_applied};

impl Client {
    /// 暂停播放。
    pub async fn pause(&self) -> Outcome<()> {
        self.ack(Request::Pause).await
    }

    /// 从暂停恢复。
    pub async fn resume(&self) -> Outcome<()> {
        self.ack(Request::Resume).await
    }

    /// 停止当前曲目。
    pub async fn stop(&self) -> Outcome<()> {
        self.ack(Request::Stop).await
    }

    /// 跳到绝对位置。
    ///
    /// # Params:
    ///   - `position_ms`: 目标位置(ms)
    pub async fn seek(&self, position_ms: u64) -> Outcome<()> {
        self.ack(Request::Seek(position_ms)).await
    }

    /// 设置音量百分比。
    ///
    /// # Params:
    ///   - `pct`: 目标音量(0..=100)
    pub async fn set_volume(&self, pct: u8) -> Outcome<()> {
        self.ack(Request::SetVolume(pct)).await
    }

    /// 循环到下一档播放模式。
    pub async fn cycle_play_mode(&self) -> Outcome<()> {
        self.ack(Request::CyclePlayMode).await
    }

    /// 直接设置播放模式。
    ///
    /// 设成当前同档时 daemon 侧是 no-op:不重洗队列、不记 `mode_changes`。
    ///
    /// # Params:
    ///   - `mode`: 目标模式
    pub async fn set_play_mode(&self, mode: PlayMode) -> Outcome<()> {
        self.ack(Request::SetPlayMode(mode)).await
    }

    /// 按当前播放模式切下一首。
    pub async fn next_song(&self) -> Outcome<()> {
        self.ack(Request::NextSong).await
    }

    /// 上一首;进度超过当前曲的重启阈值时先回开头。
    pub async fn prev_or_restart(&self) -> Outcome<()> {
        self.ack(Request::PrevOrRestart).await
    }

    /// 提交一条只需 ack 的控制请求。
    ///
    /// # Params:
    ///   - `request`: 控制请求
    async fn ack(&self, request: Request) -> Outcome<()> {
        self.request(request, decode_applied).await
    }
}

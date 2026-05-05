//! [`Server`]:audio engine + scheduler + PlayerCore + PCM puller 的单进程收纳容器。

use std::sync::Arc;

use mineral_audio::AudioHandle;
use mineral_channel_core::MusicChannel;
use mineral_task::Scheduler;
use tokio::net::UnixListener;

use crate::client::ClientHandle;
use crate::pcm::PcmPuller;
use crate::player::PlayerCore;
use crate::serve;

/// 后台 server。`spawn` 启动 audio engine + scheduler + PlayerCore + PCM puller,
/// 投递初始任务,对外发 [`ClientHandle`]。
pub struct Server {
    /// PlayerCore — 业务状态 + 自治 auto-next + events 中继。
    player: PlayerCore,

    /// PCM 中继 — 收纳 SpectrumTap,client 通过 pull_pcm 拉。
    pcm: PcmPuller,
}

impl Server {
    /// 启动 audio engine + scheduler + PlayerCore + PCM puller,投递初始任务。
    ///
    /// # Params:
    ///   - `channels`: 已构造好的全部音乐源 handle。空 vec 也合法。
    pub fn spawn(channels: Vec<Arc<dyn MusicChannel>>) -> color_eyre::Result<Self> {
        let scheduler = Scheduler::new(&channels);
        let (audio, spectrum_tap) = AudioHandle::spawn()?;
        let player = PlayerCore::spawn(audio, scheduler, channels);
        // 第一次 initial loads — 为「daemon 起来无 client 也能后台 prefetch」考虑。
        player.refresh_initial_loads();
        let pcm = PcmPuller::spawn(spectrum_tap);
        Ok(Self { player, pcm })
    }

    /// 拿一个 client handle。clone 廉价(全 Arc 内部),可任意复制给多处调用。
    pub fn client(&self) -> ClientHandle {
        ClientHandle::new(self.player.clone(), self.pcm.clone())
    }

    /// 显式 shutdown。drop 自身,利用 PlayerCore / AudioHandle / Scheduler 现有 Drop 链。
    pub fn shutdown(self) {
        drop(self);
    }

    /// IPC accept loop:每条新 connection 跑 [`mineral_protocol::Request`] dispatch。
    /// **单 client 限制**——已有 connection 时后续 incoming 立刻 `Response::Error`。
    ///
    /// 每条新 connection 接受后,内部重跑 [`PlayerCore::refresh_initial_loads`]
    /// (新 client 拿得到 PlaylistsFetched / LikedSongIdsFetched events)。
    pub async fn serve(&self, listener: UnixListener) -> color_eyre::Result<()> {
        let player = self.player.clone();
        let on_connect = move || player.refresh_initial_loads();
        serve::run(listener, self.client(), on_connect).await
    }
}

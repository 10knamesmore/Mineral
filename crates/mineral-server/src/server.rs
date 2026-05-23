//! [`Server`]:audio engine + scheduler + PlayerCore + PCM puller 的单进程收纳容器。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

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

    /// 单 client 占用标志,与 [`serve::run`] 的 accept loop 共享:心跳据此报
    /// `client_connected`(daemon 当前有没有 TUI 连着)。
    busy: Arc<AtomicBool>,
}

impl Server {
    /// 启动 audio engine + scheduler + PlayerCore + PCM puller,投递初始任务。
    ///
    /// # Params:
    ///   - `channels`: 已构造好的全部音乐源 handle。空 vec 也合法。
    pub fn spawn(channels: Vec<Arc<dyn MusicChannel>>) -> color_eyre::Result<Self> {
        mineral_log::debug!(target: "server", channels = channels.len(), "spawning server components");
        let scheduler = Scheduler::new(&channels);
        let (audio, spectrum_tap) = AudioHandle::spawn()?;
        mineral_log::debug!(target: "server", "audio engine ready");
        let player = PlayerCore::spawn(audio, scheduler, channels);
        // 第一次 initial loads — 为「daemon 起来无 client 也能后台 prefetch」考虑。
        player.refresh_initial_loads();
        let pcm = PcmPuller::spawn(spectrum_tap);
        let busy = Arc::new(AtomicBool::new(false));
        tokio::spawn(heartbeat(player.clone(), Arc::clone(&busy)));
        mineral_log::debug!(target: "server", "server components ready");
        Ok(Self { player, pcm, busy })
    }

    /// 拿一个 client handle。clone 廉价(全 Arc 内部),可任意复制给多处调用。
    pub fn client(&self) -> ClientHandle {
        ClientHandle::new(self.player.clone(), self.pcm.clone())
    }

    /// 接入系统媒体服务(Linux MPRIS):上报当前播放、响应媒体键 / 桌面控件。
    ///
    /// 仅 daemon 模式调用 —— 控制的是常驻播放。注册失败(无 D-Bus session 等)
    /// 返回 `Err`,调用方应降级而非中止 daemon。
    pub fn start_media_service(&self) -> color_eyre::Result<()> {
        crate::media::start(self.player.clone())
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
        serve::run(listener, self.client(), Arc::clone(&self.busy), on_connect).await
    }
}

/// 心跳间隔。
const HEARTBEAT_SECS: u64 = 60;

/// 心跳:每 [`HEARTBEAT_SECS`] 秒把内部状态快照打一条 info,供事后回溯间歇性问题
/// (出问题时往往没提前开 debug,有心跳就能看到那个时间点系统在干嘛)。
async fn heartbeat(player: PlayerCore, busy: Arc<AtomicBool>) {
    let start = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(HEARTBEAT_SECS));
    loop {
        tick.tick().await;
        let snap = player.snapshot();
        let audio = player.audio().snapshot();
        let tasks = player.task_snapshot();
        let (format, bitrate_kbps) = snap
            .play_url
            .as_ref()
            .map_or(("-", 0_u32), |p| (p.format.as_str(), p.bitrate_bps / 1000));
        mineral_log::info!(
            target: "heartbeat",
            uptime_s = start.elapsed().as_secs(),
            client_connected = busy.load(Ordering::Relaxed),
            playing = audio.playing,
            position_ms = audio.position_ms,
            duration_ms = audio.duration_ms,
            volume_pct = audio.volume_pct,
            song_id = snap.current_song.as_ref().map_or("-", |s| s.id.as_str()),
            play_mode = ?snap.play_mode,
            queue_len = snap.queue.len(),
            queue_sel = snap.queue_sel,
            format,
            bitrate_kbps,
            lyrics_loaded = snap.current_lyrics.is_some(),
            prefetched = player.prefetched_ready(),
            tasks_running = tasks.running,
            tasks_by_kind = ?tasks.by_kind,
            "status"
        );
    }
}

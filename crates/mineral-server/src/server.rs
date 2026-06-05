//! [`Server`]:audio engine + scheduler + PlayerCore + PCM puller 的单进程收纳容器。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mineral_audio::{AudioHandle, AudioMode};
use mineral_channel_core::MusicChannel;
use mineral_persist::ServerStore;
use mineral_task::Scheduler;
use tokio::net::UnixListener;

use crate::client::ClientHandle;
use crate::config::ServerConfig;
use crate::media_cache::MediaCache;
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
    ///   - `audio_mode`: 音频后端选择(env / config resolve 后的最终值);无设备时 `Auto` 降级而非失败。
    ///   - `persist`: 持久化句柄,透传给 [`PlayerCore::spawn`] 供后续 B-T7 起使用。
    ///   - `config`: daemon 配置切片(引擎参数 / 音质 / 缓存容量 / 各间隔)。
    pub async fn spawn(
        channels: Vec<Arc<dyn MusicChannel>>,
        audio_mode: AudioMode,
        persist: ServerStore,
        config: ServerConfig,
    ) -> color_eyre::Result<Self> {
        mineral_log::debug!(target: "server", channels = channels.len(), "spawning server components");
        let scheduler = Scheduler::new(&channels, *config.channel_workers_per());
        let (audio, spectrum_tap) = AudioHandle::spawn(audio_mode, config.engine().clone())?;
        mineral_log::debug!(target: "server", "audio engine ready");
        let media_cache = open_media_cache(&persist, *config.audio_cache_capacity()).await;
        let player = PlayerCore::spawn(audio, scheduler, channels, persist, media_cache, &config);
        // 读回上次会话 —— 本轮仅打日志确认能读到,不应用到播放状态(不自动恢复)。
        tokio::spawn(log_last_session(player.clone()));
        // 第一次 initial loads — 为「daemon 起来无 client 也能后台 prefetch」考虑。
        player.refresh_initial_loads();
        let pcm = PcmPuller::spawn(spectrum_tap);
        let busy = Arc::new(AtomicBool::new(false));
        tokio::spawn(heartbeat(
            player.clone(),
            Arc::clone(&busy),
            *config.daemon().heartbeat_secs(),
        ));
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

/// 打开音频本体缓存(`audio_cache` 表落 `persist` 的 `mineral.db`);目录解析 / open 失败时
/// `warn` 并降级到 [`MediaCache::disabled`](不阻断 daemon 启动)。
///
/// # Params:
///   - `persist`: 持久化句柄
///   - `capacity`: 缓存容量上限(字节,配置 `cache.audio_capacity`)
async fn open_media_cache(persist: &ServerStore, capacity: u64) -> MediaCache {
    let dir = match mineral_paths::audio_cache_dir() {
        Ok(d) => d,
        Err(e) => {
            mineral_log::warn!(target: "server", error = mineral_log::chain(&e), "音频缓存目录解析失败,降级禁用");
            return MediaCache::disabled();
        }
    };
    match MediaCache::open(persist, dir, capacity).await {
        Ok(cache) => cache,
        Err(e) => {
            mineral_log::warn!(target: "server", error = mineral_log::chain(&e), "音频缓存打开失败,降级禁用");
            MediaCache::disabled()
        }
    }
}

/// 启动时读回上次会话并打日志确认能读到 —— **不**应用到播放状态(本轮不自动恢复)。
/// 读不到走 debug;读出错仅 warn,不影响 daemon 启动。
async fn log_last_session(player: PlayerCore) {
    match player.load_session().await {
        Ok(Some(snap)) => mineral_log::info!(
            target: "session",
            queue_len = snap.queue.len(),
            position_ms = snap.position_ms,
            play_mode = %snap.play_mode,
            "读到上次会话"
        ),
        Ok(None) => mineral_log::debug!(target: "session", "无历史会话"),
        Err(e) => {
            mineral_log::warn!(target: "session", error = mineral_log::chain(&e), "读取上次会话失败");
        }
    }
}

/// 心跳:每 `interval_secs` 秒把内部状态快照打一条 info,供事后回溯间歇性问题
/// (出问题时往往没提前开 debug,有心跳就能看到那个时间点系统在干嘛)。
///
/// # Params:
///   - `interval_secs`: 心跳间隔(秒,配置 `daemon.heartbeat_secs`)
async fn heartbeat(player: PlayerCore, busy: Arc<AtomicBool>, interval_secs: u64) {
    let start = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_secs(interval_secs));
    loop {
        tick.tick().await;
        // in-process 直读 State 摘日志字段,不走含整队列 clone 的全量快照。
        let (song_id, play_mode, queue_len, queue_sel, format, bitrate_kbps, lyrics_loaded) =
            player.with_state(|st| {
                (
                    st.current_song
                        .as_ref()
                        .map_or_else(|| "-".to_owned(), |s| s.id.as_str().to_owned()),
                    st.play_mode,
                    st.queue.len(),
                    st.queue_sel,
                    st.play_url
                        .as_ref()
                        .map_or_else(|| "-".to_owned(), |p| p.format.as_str().to_owned()),
                    st.play_url.as_ref().map_or(0_u32, |p| p.bitrate_bps / 1000),
                    st.current_lyrics.is_some(),
                )
            });
        let audio = player.audio().snapshot();
        let tasks = player.task_snapshot();
        mineral_log::info!(
            target: "heartbeat",
            uptime_s = start.elapsed().as_secs(),
            client_connected = busy.load(Ordering::Relaxed),
            playing = audio.playing,
            position_ms = audio.position_ms,
            duration_ms = audio.duration_ms,
            volume_pct = audio.volume_pct,
            song_id,
            play_mode = ?play_mode,
            queue_len,
            queue_sel,
            format,
            bitrate_kbps,
            lyrics_loaded,
            prefetched = player.prefetched_ready(),
            tasks_running = tasks.running,
            tasks_by_kind = ?tasks.by_kind,
            "status"
        );
    }
}

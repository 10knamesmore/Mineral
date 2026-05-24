//! [`Client`] trait + [`ClientHandle`](trait 的同进程实现)。

use mineral_audio::AudioSnapshot;
use mineral_model::{MediaUrl, Song, SongId};
use mineral_protocol::{CancelFilter, PlayerSnapshot};
use mineral_task::{Priority, Snapshot, TaskEvent, TaskId, TaskKind};

use crate::pcm::PcmPuller;
use crate::player::PlayerCore;

/// 同进程 client handle:持 [`PlayerCore`] + [`PcmPuller`] 的 Arc 句柄,
/// 所有调用直接 forward。`Clone` 廉价。
#[derive(Clone)]
pub struct ClientHandle {
    /// Player 业务核心(队列/播放模式/任务调度)。
    player: PlayerCore,

    /// PCM 旁路读端,频谱 UI 用。
    pcm: PcmPuller,
}

impl ClientHandle {
    /// 同进程构造,Server 启动后用持有的 `player` / `pcm` 直接拼成 handle。
    pub(crate) fn new(player: PlayerCore, pcm: PcmPuller) -> Self {
        Self { player, pcm }
    }
}

/// Client → Server 调用面的抽象。
///
/// 现有两个实现:
/// - [`ClientHandle`]:同进程,直接转发给 [`PlayerCore`] / [`mineral_audio::AudioHandle`]
/// - `mineral_tui::remote::RemoteClient`:跨进程,内部走 unix socket
///
/// **实现方约定**:全部方法 sync。fire-and-forget 类立即返回;返回值类允许阻塞等
/// 内部 I/O,但调用方期望 < 1ms。出错时返回值类用「合理默认值」兜底。
pub trait Client: Send + Sync {
    // ---- 低级播放控制(给 SetVolume / Pause / Resume / Seek 等无业务语义的) ----
    /// 切到这个 URL,从头播。**通常 client 不应直调** —— 用 [`Self::play_song`] 让
    /// server 跑完整流程;此方法是 4b 遗留的低级入口,主要给 `mineral status` 等
    /// debug client 用。
    fn play(&self, url: MediaUrl);
    /// 暂停。
    fn pause(&self);
    /// 从暂停恢复。
    fn resume(&self);
    /// 停止当前曲目。
    fn stop(&self);
    /// 跳到绝对位置(ms)。
    fn seek(&self, position_ms: u64);
    /// 设置音量百分比(0..=100)。
    fn set_volume(&self, pct: u8);
    /// 拉一次音频快照。
    fn audio_snapshot(&self) -> AudioSnapshot;

    // ---- Player 业务 ----
    /// Client 选了一首歌。Server 跑完整 play 流程(stop + cancel + 命中 prefetch /
    /// submit SongUrl + submit Lyrics)。
    fn play_song(&self, song: Song);

    /// 替换 queue + 设当前位置。Shuffle 模式下 server 端洗牌。
    fn set_queue(&self, queue: Vec<Song>, target_id: SongId);

    /// `m` 键:cycle PlayMode。
    fn cycle_play_mode(&self);

    /// `p` 键:进度 > 阈值时回开头,否则跳上一首。
    fn prev_or_restart(&self);

    /// `n` 键:按 PlayMode 切下一首。
    fn next_song(&self);

    /// 拉一份 PlayerSnapshot;client 启动 / 重连时灌进 UI。
    fn player_snapshot(&self) -> PlayerSnapshot;

    // ---- 任务调度(直通,playlists/tracks 类 prefetch 用) ----
    /// 提交一个任务,返回任务 id。
    fn submit_task(&self, kind: TaskKind, priority: Priority) -> TaskId;
    /// 按 [`CancelFilter`] 批量取消。
    fn cancel_tasks(&self, filter: CancelFilter);
    /// 拉走 server 端积攒的任务事件(已 filter,不含 PlayUrlReady/LyricsReady)。
    fn drain_task_events(&self) -> Vec<TaskEvent>;
    /// 当前 scheduler 状态快照。
    fn task_snapshot(&self) -> Snapshot;

    // ---- PCM 流(client spectrum 用) ----
    /// 拉最多 N 个 PCM sample。返回 (samples, sample_rate);可能短于 N。
    fn pull_pcm(&self, n: usize) -> (Vec<f32>, u32);

    /// client 与 server 的链路是否仍可用。
    ///
    /// 同进程实现([`ClientHandle`])恒 `true`(client 与 server 同生共死)。
    /// 跨进程实现(`RemoteClient`)在 daemon 断开后返回 `false`,UI 据此干净退出
    /// 而非僵死在「所有请求兜底默认值」的状态。默认实现返回 `true`。
    fn connected(&self) -> bool {
        true
    }
}

impl Client for ClientHandle {
    fn play(&self, url: MediaUrl) {
        // PlayerCore 内部的 audio handle 暴露不出来直接调,留给 server-internal
        // 使用 (PlayUrlReady → audio.play)。client 这条路径目前只在 status 之类
        // 工具里调,直接 fire 给 audio (经 player) 是 OK 的:暂以 stop 兼容,
        // 真要播请走 play_song。
        // ——但 mineral status 也不调这个。本方法保留不删,日后 debug 可用。
        let _ = url;
    }
    fn pause(&self) {
        // pause/resume/stop/seek/set_volume 仍直通 audio。PlayerCore 内部持 audio
        // 但没暴露;通过新加的 audio() getter 走。
        self.player.audio().pause();
    }
    fn resume(&self) {
        self.player.audio().resume();
    }
    fn stop(&self) {
        self.player.audio().stop();
    }
    fn seek(&self, position_ms: u64) {
        self.player.audio().seek(position_ms);
    }
    fn set_volume(&self, pct: u8) {
        self.player.audio().set_volume(pct);
    }
    fn audio_snapshot(&self) -> AudioSnapshot {
        self.player.audio().snapshot()
    }

    fn play_song(&self, song: Song) {
        self.player.play_song(&song);
    }
    fn set_queue(&self, queue: Vec<Song>, target_id: SongId) {
        self.player.set_queue(queue, &target_id);
    }
    fn cycle_play_mode(&self) {
        self.player.cycle_play_mode();
    }
    fn prev_or_restart(&self) {
        self.player.prev_or_restart();
    }
    fn next_song(&self) {
        self.player.next_song();
    }
    fn player_snapshot(&self) -> PlayerSnapshot {
        self.player.snapshot()
    }

    fn submit_task(&self, kind: TaskKind, priority: Priority) -> TaskId {
        self.player.submit_task(kind, priority)
    }
    fn cancel_tasks(&self, filter: CancelFilter) {
        self.player.cancel_tasks_where(move |k| filter.matches(k));
    }
    fn drain_task_events(&self) -> Vec<TaskEvent> {
        self.player.drain_client_events()
    }
    fn task_snapshot(&self) -> Snapshot {
        self.player.task_snapshot()
    }

    fn pull_pcm(&self, n: usize) -> (Vec<f32>, u32) {
        self.pcm.pull(n)
    }
}

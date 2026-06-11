//! [`Client`] trait + [`ClientHandle`](trait 的同进程实现)。

use mineral_audio::AudioSnapshot;
use mineral_model::{MediaUrl, Song, SongId};
use mineral_protocol::{
    CancelFilter, DownloadProgress, DownloadTarget, Event, PlayerSync, PlayerVersions,
    SongStatsWire,
};
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

    /// event hub 订阅端(in-proc 推送通路:Toast / StoreChanged 等;
    /// 多 clone 共享同一订阅,与「单 client」语义一致)。
    events: std::sync::Arc<parking_lot::Mutex<tokio::sync::broadcast::Receiver<Event>>>,
}

impl ClientHandle {
    /// 同进程构造,Server 启动后用持有的 `player` / `pcm` 直接拼成 handle。
    pub(crate) fn new(
        player: PlayerCore,
        pcm: PcmPuller,
        events: tokio::sync::broadcast::Receiver<Event>,
    ) -> Self {
        Self {
            player,
            pcm,
            events: std::sync::Arc::new(parking_lot::Mutex::new(events)),
        }
    }

    /// 切换一首歌的 love(♥)状态:查当前态 → 经对应 channel `set_loved`
    /// (本地 persist + 远端)→ 返回切换后的新态。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲 id;其 namespace 决定走哪个 channel / persist scope。
    ///
    /// # Return:
    ///   切换后的新 loved 状态。
    pub(crate) async fn toggle_love_async(&self, id: &SongId) -> color_eyre::Result<bool> {
        let ns = id.namespace();
        let current = self.player.persist().scope(ns).is_loved(id).await?;
        let new = !current;
        let channel = self
            .player
            .channel_for(ns)
            .ok_or_else(|| color_eyre::eyre::eyre!("no channel for source {}", ns.name()))?
            .clone();
        channel
            .set_loved(id, new)
            .await
            .map_err(|e| color_eyre::eyre::eyre!("set_loved failed: {e}"))?;
        Ok(new)
    }

    /// 触发脚本具名动作并等待结果(serve 层处理 `InvokeAction` 用)。
    ///
    /// # Params:
    ///   - `name`: 动作注册名
    ///   - `ctx`: 按键瞬间的 client 上下文(无界面触发面为 `None`)
    ///
    /// # Return:
    ///   成功为 `Ok`;脚本未启用 / 未注册 / 执行失败为 `Err`。
    pub(crate) async fn invoke_action_async(
        &self,
        name: &str,
        ctx: Option<mineral_protocol::KeyContext>,
    ) -> color_eyre::Result<()> {
        self.player.invoke_script_action(name, ctx).await
    }

    /// 读 per-song 持久值(serve 层处理 `StoreGet` 用)。未命中返回 `Nil`。
    ///
    /// # Params:
    ///   - `id`: 目标歌;其 namespace 决定 persist scope
    ///   - `key`: 开放键
    pub(crate) async fn store_get_async(
        &self,
        id: &SongId,
        key: &str,
    ) -> color_eyre::Result<mineral_protocol::StoreValue> {
        self.player
            .persist()
            .scope(id.namespace())
            .kv_get(id, key)
            .await
    }

    /// 写 per-song 持久值(serve 层处理 `StoreSet` 用);写成功推 `StoreChanged`。
    ///
    /// # Params:
    ///   - `id`: 目标歌
    ///   - `key`: 开放键(保留键拒写,错误冒泡给 client)
    ///   - `value`: 标量值(`Nil` 删除)
    pub(crate) async fn store_set_async(
        &self,
        id: &SongId,
        key: &str,
        value: &mineral_protocol::StoreValue,
    ) -> color_eyre::Result<()> {
        self.player
            .persist()
            .scope(id.namespace())
            .kv_set(id, key, value)
            .await?;
        self.player.notify().store_changed(id, key);
        Ok(())
    }

    /// 覆盖表快照(serve 层握手订阅 `UiOverride` 时逐条重放)。
    pub(crate) fn ui_overrides_snapshot(&self) -> Vec<(String, mineral_protocol::BusValue)> {
        self.player.ui_overrides_snapshot()
    }

    /// client 断开时清空终端状态(serve 层连接收尾调;`terminal` 属性回 None)。
    pub(crate) fn clear_terminal_state(&self) {
        self.player.clear_terminal_state();
    }

    /// 拉取脚本 bind 表(serve 层处理 `ScriptBinds` 用);无脚本 / 线程退出为空。
    pub(crate) async fn script_binds_async(&self) -> Vec<mineral_protocol::ScriptBind> {
        let Some(script) = self.player.script_sender() else {
            return Vec::new();
        };
        script.script_binds().await.unwrap_or_default()
    }

    /// per-song 数值自增(serve 层处理 `StoreInc` 用);成功推 `StoreChanged`。
    ///
    /// # Params:
    ///   - `id`: 目标歌
    ///   - `key`: 开放键
    ///   - `delta`: 增量(可负)
    ///
    /// # Return:
    ///   自增后的值。
    pub(crate) async fn store_inc_async(
        &self,
        id: &SongId,
        key: &str,
        delta: i64,
    ) -> color_eyre::Result<mineral_protocol::StoreValue> {
        let value = self
            .player
            .persist()
            .scope(id.namespace())
            .kv_inc(id, key, delta)
            .await?;
        self.player.notify().store_changed(id, key);
        Ok(value)
    }

    /// 查询一首歌的播放统计(persist),转成 protocol DTO。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲 id;其 namespace 决定 persist scope。
    ///
    /// # Return:
    ///   命中返回 [`mineral_protocol::SongStatsWire`],无记录返回 `None`。
    pub(crate) async fn query_song_stats_async(
        &self,
        id: &SongId,
    ) -> color_eyre::Result<Option<mineral_protocol::SongStatsWire>> {
        let stats = self
            .player
            .persist()
            .scope(id.namespace())
            .query_stats(id)
            .await?;
        Ok(stats.map(|s| mineral_protocol::SongStatsWire {
            play_count: s.play_count,
            skip_count: s.skip_count,
            total_listen_ms: s.total_listen_ms,
            last_played_at: s.last_played_at,
            loved: s.loved,
        }))
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

    /// 版本门控的播放状态同步:`known` 是 client 已持有的版本号(0 = 一无所有),
    /// server 仅对落后部分附带重段。启动与每 tick 同一条路径(语义见 [`PlayerSync`])。
    fn player_sync(&self, known: PlayerVersions) -> PlayerSync;

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

    // ---- 喜欢 / 统计 ----
    /// 切换一首歌的喜欢(♥)状态,返回切换后的新 loved 态。
    ///
    /// daemon 模式经 IPC 拿到真实结果;in-proc 模式 fire-and-forget,返回值为乐观占位
    /// (调用方 TUI 应自行乐观更新本地 loved 态,不强依赖此返回值)。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲 id。
    ///
    /// # Return:
    ///   切换后的 loved 状态(daemon 模式为真实值;in-proc 为占位 `false`)。
    fn toggle_love(&self, id: SongId) -> bool;

    /// 触发脚本具名动作(`mineral.action` 注册)。
    ///
    /// # Params:
    ///   - `name`: 动作注册名。
    ///   - `ctx`: 按键瞬间的 client 上下文(无界面 / 采不到传 `None`)。
    ///
    /// # Return:
    ///   `None` = 已受理 / 成功;`Some(err)` = daemon 报错(未注册 / 脚本未启用 /
    ///   执行失败),client 应提示用户。
    fn invoke_action(
        &self,
        name: &str,
        ctx: Option<mineral_protocol::KeyContext>,
    ) -> Option<String> {
        let _ = (name, ctx);
        Some("脚本动作不可用(当前 client 不支持)".to_owned())
    }

    /// 拉取脚本 `mineral.bind` 的键绑定表(client 启动 / 配置重载后调,
    /// 合进自己的 keymap)。
    ///
    /// 默认空(in-proc 调试模式不起脚本线程,空表即正确语义);daemon 模式
    /// 经 IPC 拿真表。
    fn script_binds(&self) -> Vec<mineral_protocol::ScriptBind> {
        Vec::new()
    }

    /// 查询一首歌的播放统计;无记录 / 不可用返回 `None`。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲 id。
    ///
    /// # Return:
    ///   有记录时返回 [`SongStatsWire`],否则 `None`。
    fn query_song_stats(&self, id: SongId) -> Option<SongStatsWire>;

    /// 下载(永久导出 + 顺带填 cache)单曲 / 整张歌单。fire-and-forget,server 后台跑,
    /// 进度 / 完成经 [`TaskEvent::Notice`] 回传(client 拉 task events 时取到)。
    ///
    /// # Params:
    ///   - `target`: 下载目标(单曲 / 歌单)
    fn download(&self, target: DownloadTarget);

    /// 拉一次下载进度快照(TUI 进度弹窗 / CLI status 用)。无下载时 `active == false`。
    fn download_progress(&self) -> DownloadProgress;

    /// 上报终端 UI 状态(resize / 全屏切换时调;值没变 client 侧应去抖不发)。
    /// daemon 灌属性树 `terminal` 复合属性供脚本 observe。fire-and-forget。
    ///
    /// 默认 no-op(无界面 / 测试 client 不上报)。
    ///
    /// # Params:
    ///   - `rows`: 终端行数
    ///   - `cols`: 终端列数
    ///   - `fullscreen`: 是否处于全屏播放态
    ///   - `focused`: 终端窗口是否持有输入焦点
    fn report_terminal_state(&self, rows: u16, cols: u16, fullscreen: bool, focused: bool) {
        let _ = (rows, cols, fullscreen, focused);
    }

    /// client 与 server 的链路是否仍可用。
    ///
    /// 同进程实现([`ClientHandle`])恒 `true`(client 与 server 同生共死)。
    /// 跨进程实现(`RemoteClient`)在 daemon 断开后返回 `false`,UI 据此干净退出
    /// 而非僵死在「所有请求兜底默认值」的状态。默认实现返回 `true`。
    fn connected(&self) -> bool {
        true
    }

    /// 请求 daemon 优雅退出(TUI「退出并停止 daemon」用)。
    ///
    /// 默认 no-op——同进程实现([`ClientHandle`])下 TUI 与 server 同生共死,
    /// 进程退 = server drop,不存在可独立停掉的 daemon。跨进程实现
    /// (`RemoteClient`)经 IPC 发 `Request::Shutdown`;断连时静默失败
    /// (调用方已在退出路径上,无从补救也无需提示)。
    fn request_daemon_shutdown(&self) {}

    /// 拉走 server 主动推送的 [`mineral_protocol::Event`](与轮询式
    /// [`Self::drain_task_events`] 是两条通道:这条是握手订阅后 server 随时下推、
    /// client 侧缓冲,每 tick drain)。
    ///
    /// 跨进程实现(`RemoteClient`)返回缓冲的推送;同进程 / 测试实现用默认空
    /// (in-proc 无推送通道,Phase 1 也没有 daemon 内生产者面向 in-proc)。
    fn drain_events(&self) -> Vec<mineral_protocol::Event> {
        Vec::new()
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
        self.player.stop_playback();
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
    fn player_sync(&self, known: PlayerVersions) -> PlayerSync {
        self.player.sync(known)
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

    fn drain_events(&self) -> Vec<Event> {
        let mut rx = self.events.lock();
        let mut events = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                // 积压被挤掉(in-proc 每 tick drain,正常到不了):跳过继续收。
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(skipped)) => {
                    mineral_log::warn!(target: "client", skipped, "event hub 积压,推送被丢弃");
                }
                Err(_empty_or_closed) => return events,
            }
        }
    }
    fn task_snapshot(&self) -> Snapshot {
        self.player.task_snapshot()
    }

    fn pull_pcm(&self, n: usize) -> (Vec<f32>, u32) {
        self.pcm.pull(n)
    }

    fn toggle_love(&self, id: SongId) -> bool {
        // in-proc 降级:fire-and-forget 触发完整 toggle(查+翻转+set_loved),返回乐观占位。
        // TUI 会乐观更新本地态,不依赖此返回值。
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.toggle_love_async(&id).await {
                mineral_log::warn!(
                    target: "client",
                    error = mineral_log::chain(&e),
                    "in-proc toggle_love 失败"
                );
            }
        });
        false // 占位,不保证准确
    }

    fn query_song_stats(&self, _id: SongId) -> Option<SongStatsWire> {
        // in-proc 调试模式不支持同步查统计(无法 block on async),返回 None。
        None
    }

    fn download(&self, target: DownloadTarget) {
        self.player.download(target);
    }

    fn download_progress(&self) -> DownloadProgress {
        self.player.download_progress()
    }

    fn report_terminal_state(&self, rows: u16, cols: u16, fullscreen: bool, focused: bool) {
        self.player
            .set_terminal_state(crate::props::TerminalReport {
                rows,
                cols,
                fullscreen,
                focused,
            });
    }
}

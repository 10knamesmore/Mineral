//! [`Client`] trait + [`ClientHandle`](trait 的同进程实现)。

use mineral_audio::AudioSnapshot;
use mineral_channel_core::ChannelCaps;
use mineral_model::{MediaUrl, Song, SongId, SourceKind};
use mineral_protocol::{
    CancelFilter, DownloadProgress, DownloadTarget, Event, PlayerSync, PlayerVersions,
    QueueContextWire, SongStatsWire,
};
use mineral_task::{Priority, Snapshot, TaskEvent, TaskId, TaskKind};

use crate::pcm::PcmPuller;
use crate::player::PlayerCore;

/// wire 队列语境 → 埋点 [`mineral_stats::QueueContext`](边缘适配:protocol 不依赖 stats)。
fn queue_context_from_wire(wire: QueueContextWire) -> mineral_stats::QueueContext {
    use mineral_stats::QueueContext;
    match wire {
        // wire 侧总带原文;落库前由 recorder 按 search_queries 隐私档 redact。
        QueueContextWire::Search { query } => QueueContext::Search { query: Some(query) },
        QueueContextWire::Playlist { id } => QueueContext::Playlist { id },
        QueueContextWire::Album { id } => QueueContext::Album { id },
        QueueContextWire::Artist { id } => QueueContext::Artist { id },
        QueueContextWire::Manual => QueueContext::Manual,
        QueueContextWire::Unknown => QueueContext::Unknown,
    }
}

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

    /// 切换一首歌的 love(♥)状态:本地 persist 事实来源必写 + 尽力镜像远端,返回切换后的新态。
    /// 编排见 [`PlayerCore::toggle_favorite`](crate::player::PlayerCore)。
    ///
    /// # Params:
    ///   - `song`: 目标歌曲(整首传入,server 顺手落 meta 供聚合视图重建)。
    ///
    /// # Return:
    ///   切换后的新 loved 状态。
    pub(crate) async fn toggle_love_async(&self, song: &Song) -> color_eyre::Result<bool> {
        // 埋点在 toggle_favorite 单点(client / 脚本入口同享),此处只穿透 actor。
        self.player
            .toggle_favorite(song, mineral_stats::Actor::User)
            .await
    }

    /// 记一次连接拒绝(connection_rejects)。actor=System:daemon 主动拒外来连接,
    /// 非任何 user/script/cli 发起,归系统。
    ///
    /// # Params:
    ///   - `reason`: 拒绝原因(busy / 版本不匹配)
    pub(crate) fn record_connection_reject(&self, reason: mineral_stats::RejectReason) {
        self.player
            .inner
            .stats
            .event(mineral_stats::StatsEvent::Behavior {
                actor: mineral_stats::Actor::System,
                event: mineral_stats::BehaviorEvent::ConnectionReject { reason },
            });
    }

    /// 记一次行为域事件(user 发起,stamp now_ms)。
    fn record_behavior(&self, event: mineral_stats::BehaviorEvent) {
        self.player
            .inner
            .stats
            .event(mineral_stats::StatsEvent::Behavior {
                actor: mineral_stats::Actor::User,
                event,
            });
    }

    /// 触发脚本具名动作并等待结果(serve 层处理 `InvokeAction` 用)。
    ///
    /// # Params:
    ///   - `name`: 动作注册名
    ///   - `ctx`: 按键瞬间的 client 上下文(无界面触发面为 `None`)
    ///   - `args`: 调用位置实参(CLI 采集;无参触发为空)
    ///
    /// # Return:
    ///   成功为 `Ok`;脚本未启用 / 未注册 / 执行失败为 `Err`。
    pub(crate) async fn invoke_action_async(
        &self,
        name: &str,
        ctx: Option<mineral_protocol::KeyContext>,
        args: Vec<String>,
    ) -> color_eyre::Result<()> {
        // trigger:带 KeyContext = TUI 键触发;无 = CLI `mineral action`。
        let trigger = if ctx.is_some() {
            mineral_stats::ActionTrigger::Tui
        } else {
            mineral_stats::ActionTrigger::Cli
        };
        let result = self.player.invoke_script_action(name, ctx, args).await;
        self.record_behavior(mineral_stats::BehaviorEvent::ActionInvocation {
            name: name.to_owned(),
            trigger,
            outcome: if result.is_ok() {
                mineral_stats::OpOutcome::Ok
            } else {
                mineral_stats::OpOutcome::Failed
            },
        });
        result
    }

    /// 渲染一个复制模板并等待结果(serve 层处理 `RenderCopyTemplate` 用)。
    ///
    /// # Params:
    ///   - `index`: 模板下标(0-based,对位 config `copy.templates` 数组序)
    ///   - `ctx`: 模板作用的实体
    ///
    /// # Return:
    ///   `Ok(text)` = 剪贴板文本;`Err(msg)` = 人读错误。
    pub(crate) async fn render_copy_template_async(
        &self,
        index: usize,
        ctx: mineral_protocol::CopyTemplateCtx,
    ) -> Result<String, String> {
        // 埋点前先据 ctx 取类型 + 目标(ctx 随即被 render 移走)。
        let (ctx_kind, target_ref) = match &ctx {
            mineral_protocol::CopyTemplateCtx::Song(s) => {
                (mineral_stats::CopyContext::Song, Some(s.id.qualified()))
            }
            mineral_protocol::CopyTemplateCtx::Playlist(p) => {
                (mineral_stats::CopyContext::Playlist, Some(p.id.qualified()))
            }
        };
        let result = self.player.render_copy_template(index, ctx).await;
        // 埋点:文案渲染(copy_renders;user 发起)。Err = 模板缺失 / 渲染失败。
        self.record_behavior(mineral_stats::BehaviorEvent::CopyRender {
            template_index: i64::try_from(index).unwrap_or(i64::MAX),
            ctx_kind,
            target_ref,
            outcome: if result.is_ok() {
                mineral_stats::OpOutcome::Ok
            } else {
                mineral_stats::OpOutcome::Failed
            },
        });
        result
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

    /// 当前有效配置(serve 层握手订阅 `Config` 时重放一帧)。
    pub(crate) fn effective_config(&self) -> mineral_protocol::BusValue {
        self.player.effective_config()
    }

    /// 当前窗口标题覆盖(serve 层握手订阅 `WindowTitle` 时重放;无覆盖不发)。
    pub(crate) fn window_title_override(&self) -> Option<String> {
        self.player.window_title_override()
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
        // play/skip/listen/last 改由 stats.db 聚合(全量窗口);loved 是功能状态,仍读
        // mineral.db。窗口语义与现状一致,wire 类型不变、client 零改动。
        let summary = self.player.inner.stats.store().song_summary(id).await?;
        let loved = self
            .player
            .persist()
            .scope(id.namespace())
            .query_stats(id)
            .await?
            .is_some_and(|s| s.loved);
        if summary.is_none() && !loved {
            return Ok(None);
        }
        let wire = match summary {
            Some(s) => mineral_protocol::SongStatsWire {
                play_count: u32::try_from(s.plays).unwrap_or(u32::MAX),
                skip_count: u32::try_from(s.skips).unwrap_or(u32::MAX),
                total_listen_ms: u64::try_from(s.listen_ms).unwrap_or(0),
                last_played_at: s.last_played_at,
                loved,
            },
            None => mineral_protocol::SongStatsWire {
                play_count: 0,
                skip_count: 0,
                total_listen_ms: 0,
                last_played_at: None,
                loved,
            },
        };
        Ok(Some(wire))
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
    ///
    /// # Params:
    ///   - `queue`: 新队列
    ///   - `target_id`: 队列中作为「当前」的歌
    ///   - `context`: 队列语境(埋点 provenance:该队列来自搜索 / 歌单 / 专辑 / 艺人 / 手动)
    fn set_queue(&self, queue: Vec<Song>, target_id: SongId, context: QueueContextWire);

    /// 插播:插到当前曲之后,不动队列级 context 与当前曲。
    ///
    /// # Params:
    ///   - `song`: 待插播的歌
    ///   - `context`: 该曲来源语境(埋点 per-song 覆盖:插队散曲不继承队列级 context)
    fn queue_insert_next(&self, song: Song, context: QueueContextWire);

    /// 追加到队列末尾,不动队列级 context 与当前曲。
    ///
    /// # Params:
    ///   - `song`: 待追加的歌
    ///   - `context`: 该曲来源语境(埋点 per-song 覆盖:同插播)
    fn queue_append(&self, song: Song, context: QueueContextWire);

    /// 全部已注册 channel 的能力声明(启动握手拉一次,断连重连后再拉)。
    fn channel_caps(&self) -> Vec<(SourceKind, ChannelCaps)>;

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
    ///   - `song`: 目标歌曲(整首传入,server 顺手落 meta 供聚合视图重建)。
    ///
    /// # Return:
    ///   切换后的 loved 状态(daemon 模式为真实值;in-proc 为占位 `false`)。
    fn toggle_love(&self, song: Song) -> bool;

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

    /// 渲染一个复制模板(daemon 脚本运行时执行 config `copy.templates[index]`
    /// 的函数)。默认不可用(in-proc 调试模式无脚本线程);daemon 模式经 IPC。
    ///
    /// # Params:
    ///   - `index`: 模板下标(0-based,对位 config 数组序)。
    ///   - `ctx`: 模板作用的实体。
    ///
    /// # Return:
    ///   `Ok(text)` = 进剪贴板的文本;`Err(msg)` = 人读错误,client 应 toast。
    fn render_copy_template(
        &self,
        index: usize,
        ctx: mineral_protocol::CopyTemplateCtx,
    ) -> Result<String, String> {
        let _ = (index, ctx);
        Err("复制模板不可用(当前 client 不支持)".to_owned())
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
        // 传输面(执行 + 埋点)统一走 PlayerCore 的 transport 方法,client 只穿透 actor。
        self.player.pause_playback(mineral_stats::Actor::User);
    }
    fn resume(&self) {
        self.player.resume_playback(mineral_stats::Actor::User);
    }
    fn stop(&self) {
        self.player.stop_playback();
    }
    fn seek(&self, position_ms: u64) {
        self.player
            .seek_playback(position_ms, mineral_stats::Actor::User);
    }
    fn set_volume(&self, pct: u8) {
        self.player
            .set_playback_volume(pct, mineral_stats::Actor::User);
    }
    fn audio_snapshot(&self) -> AudioSnapshot {
        self.player.audio().snapshot()
    }

    fn play_song(&self, song: Song) {
        // 直接改播会顶掉在播曲:先按 skip 结算它(next/prev/EOF 各有自己的结算,唯独这
        // 条显式点播路径需要在此结算,否则被打断曲的 plays 行丢失)。
        self.player.settle_interrupted();
        self.player.play_song(
            &song,
            mineral_stats::PlayOrigin::Explicit,
            mineral_stats::Actor::User,
        );
    }
    fn set_queue(&self, queue: Vec<Song>, target_id: SongId, context: QueueContextWire) {
        let count = i64::try_from(queue.len()).unwrap_or(i64::MAX);
        self.player
            .set_queue(queue, &target_id, queue_context_from_wire(context));
        self.record_behavior(mineral_stats::BehaviorEvent::QueueOp {
            op: mineral_stats::QueueOp::Set,
            song: None,
            count,
        });
    }
    fn queue_insert_next(&self, song: Song, context: QueueContextWire) {
        let id = song.id.clone();
        self.player
            .queue_insert_next(song, queue_context_from_wire(context));
        self.record_behavior(mineral_stats::BehaviorEvent::QueueOp {
            op: mineral_stats::QueueOp::InsertNext,
            song: Some(id),
            count: 1,
        });
    }
    fn queue_append(&self, song: Song, context: QueueContextWire) {
        let id = song.id.clone();
        self.player
            .queue_append(song, queue_context_from_wire(context));
        self.record_behavior(mineral_stats::BehaviorEvent::QueueOp {
            op: mineral_stats::QueueOp::Append,
            song: Some(id),
            count: 1,
        });
    }
    fn channel_caps(&self) -> Vec<(SourceKind, ChannelCaps)> {
        self.player.channel_caps()
    }
    fn cycle_play_mode(&self) {
        // mode_changes 埋点在 PlayerCore 单点(cycle / 直设 / 脚本共用)。
        self.player.cycle_play_mode(mineral_stats::Actor::User);
    }
    fn prev_or_restart(&self) {
        self.player.prev_or_restart(mineral_stats::Actor::User);
    }
    fn next_song(&self) {
        self.player.next_song(mineral_stats::Actor::User);
    }
    fn player_sync(&self, known: PlayerVersions) -> PlayerSync {
        self.player.sync(known)
    }

    fn submit_task(&self, kind: TaskKind, priority: Priority) -> TaskId {
        self.player.submit_task(kind, priority)
    }
    fn cancel_tasks(&self, filter: CancelFilter) {
        let filter_tags = match &filter {
            CancelFilter::ChannelFetchKinds(tags) => tags
                .iter()
                .map(|t| format!("{t:?}"))
                .collect::<Vec<_>>()
                .join(","),
        };
        self.player.cancel_tasks_where(move |k| filter.matches(k));
        self.record_behavior(mineral_stats::BehaviorEvent::TaskCancel { filter_tags });
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

    fn toggle_love(&self, song: Song) -> bool {
        // in-proc 降级:fire-and-forget 触发完整 toggle(查+翻转+set_loved),返回乐观占位。
        // TUI 会乐观更新本地态,不依赖此返回值。
        let this = self.clone();
        tokio::spawn(async move {
            if let Err(e) = this.toggle_love_async(&song).await {
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
        let toggled = self
            .player
            .set_terminal_state(crate::props::TerminalReport {
                rows,
                cols,
                fullscreen,
                focused,
            });
        // 埋点:全屏切换(fullscreen_changes;仅在相对前态翻转时,滤掉每 tick 的等值上报)。
        if let Some(fullscreen) = toggled {
            self.record_behavior(mineral_stats::BehaviorEvent::FullscreenChange { fullscreen });
        }
    }
}

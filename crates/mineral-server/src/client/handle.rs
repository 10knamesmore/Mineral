//! [`ClientHandle`]:[`Client`] 契约的同进程实现。

use mineral_audio::AudioSnapshot;
use mineral_channel_core::ChannelCaps;
use mineral_model::{MediaUrl, Song, SongId, SourceKind};
use mineral_protocol::{
    CancelFilter, DownloadProgress, DownloadTarget, Event, PlayerSync, PlayerVersions,
    QueueContextWire, QueueEditOutcome, QueueOp, SongStatsWire,
};
use mineral_task::{Priority, Snapshot, TaskEvent, TaskId, TaskKind};

use super::contract::Client;
use super::wire::{edited_song_id, queue_context_from_wire, stats_queue_op};
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
    /// 多 clone 共享同一订阅——in-proc 只有一个消费者)。
    events: std::sync::Arc<parking_lot::Mutex<tokio::sync::broadcast::Receiver<Event>>>,

    /// 本 handle 归属的连接 id:wire 接入经 [`Self::for_connection`] 每连接
    /// 唯一,per-conn 状态(终端上报 / PCM 游标)以它归属;in-proc 恒 0。
    conn: u64,
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
            conn: 0,
        }
    }

    /// 派生某条 wire 连接专属的 handle(serve 层 accept 时调)。
    ///
    /// # Params:
    ///   - `conn`: 连接 id(注册表分配,进程内唯一)
    pub(crate) fn for_connection(&self, conn: u64) -> Self {
        let mut handle = self.clone();
        handle.conn = conn;
        handle
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

    /// 记一次 client 连接生命周期(client_connections;断开时调,只记握手
    /// 完成的连接)。actor=User:连接由用户起的 client 发起。
    ///
    /// # Params:
    ///   - `client`: client 自报名(握手 `ClientInfo::name`)
    ///   - `duration_ms`: 连接存续时长
    ///   - `concurrent`: 建立时刻在线连接数(含自己)
    pub(crate) fn record_client_connection(
        &self,
        client: String,
        duration_ms: i64,
        concurrent: i64,
    ) {
        self.record_behavior(mineral_stats::BehaviorEvent::ClientConnection {
            client,
            duration_ms,
            concurrent,
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
            mineral_protocol::CopyTemplateCtx::Album(a) => {
                (mineral_stats::CopyContext::Album, Some(a.id.qualified()))
            }
            mineral_protocol::CopyTemplateCtx::Artist(a) => {
                (mineral_stats::CopyContext::Artist, Some(a.id.qualified()))
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

    /// 队列编辑的完整入口(serve 层用)。
    ///
    /// [`QueueOp::ApplyTransform`] 要跨线程跑脚本、必须异步,其余操作同步落地。变换失败
    /// (脚本未启用 / 报错 / 超时 / 返回未知 id)一律 fail-open:队列不动 + toast 提示,
    /// 不把用户的队列丢在半路。
    ///
    /// # Params:
    ///   - `op`: 待执行的操作
    ///
    /// # Return:
    ///   本次编辑的结果。
    pub(crate) async fn queue_edit_async(&self, op: QueueOp) -> QueueEditOutcome {
        let QueueOp::ApplyTransform { index, selected } = op else {
            return <Self as Client>::queue_edit(self, op);
        };
        let (queue, current) = self
            .player
            .with_state(|st| (st.queue.clone(), st.cursor.anchor()));
        let after = queue.len();
        match self
            .player
            .queue_transform(index, queue, current, selected)
            .await
        {
            Ok(ids) => {
                let outcome = self.player.queue_reorder(&ids);
                if matches!(outcome, QueueEditOutcome::Stale) {
                    self.player.notify().toast(
                        mineral_protocol::ToastKind::Warn,
                        "queue transform returned a song outside the queue".to_owned(),
                    );
                }
                if matches!(outcome, QueueEditOutcome::Applied) {
                    self.record_behavior(mineral_stats::BehaviorEvent::QueueOp {
                        op: mineral_stats::QueueOp::Transform,
                        song: None,
                        count: i64::try_from(after).unwrap_or(i64::MAX),
                    });
                }
                outcome
            }
            Err(message) => {
                mineral_log::warn!(
                    target: "script",
                    index,
                    error = message.as_str(),
                    "queue transform failed, leaving the queue untouched"
                );
                self.player
                    .notify()
                    .toast(mineral_protocol::ToastKind::Warn, message);
                QueueEditOutcome::NoOp
            }
        }
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

    /// 按握手订阅集组装重放帧:各订阅类别的当前状态快照,先于实时流下发,
    /// 新 client 无须等待下一次变更即拿到完整现状。
    ///
    /// # Params:
    ///   - `subscriptions`: 握手声明的订阅集
    ///
    /// # Return:
    ///   重放帧序列(无需重放的类别不产帧)。
    pub(crate) async fn replay_frames(
        &self,
        subscriptions: &[mineral_protocol::Subscription],
    ) -> Vec<Event> {
        use mineral_protocol::Subscription;
        let mut frames = Vec::new();
        if subscriptions.contains(&Subscription::Config) {
            frames.push(Event::ConfigChanged {
                config: self.effective_config(),
            });
        }
        if subscriptions.contains(&Subscription::WindowTitle)
            && let Some(text) = self.window_title_override()
        {
            frames.push(Event::WindowTitleOverride { text: Some(text) });
        }
        if subscriptions.contains(&Subscription::Task) {
            if let Some(playlists) = self.player.library().cached_snapshot() {
                frames.push(Event::Task(Box::new(TaskEvent::LibrarySnapshot {
                    playlists,
                })));
            }
            for (source, ids) in self.player.favorited_ids_by_source().await {
                frames.push(Event::Task(Box::new(TaskEvent::LikedSongIdsFetched {
                    source,
                    ids,
                })));
            }
        }
        frames
    }

    /// 本连接断开的收尾(serve 层连接收尾调):移除其终端上报(全部离线时
    /// `terminal` 属性回 None)与 PCM 游标。
    pub(crate) fn connection_closed(&self) {
        self.player.clear_terminal_state(self.conn);
        self.pcm.drop_cursor(self.conn);
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
    fn queue_edit(&self, op: QueueOp) -> QueueEditOutcome {
        let before = self.player.with_state(|st| st.queue.len());
        let outcome = self.player.queue_edit(&op);
        if matches!(outcome, QueueEditOutcome::Applied) {
            let after = self.player.with_state(|st| st.queue.len());
            self.record_behavior(mineral_stats::BehaviorEvent::QueueOp {
                op: stats_queue_op(&op),
                song: edited_song_id(&op),
                // 纯重排不改长度,记 1 条「受影响」;批量清理记实际删除条数。
                count: i64::try_from(before.abs_diff(after).max(1)).unwrap_or(i64::MAX),
            });
        }
        outcome
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
        self.pcm.pull(self.conn, n)
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
        let toggled = self.player.set_terminal_state(
            self.conn,
            crate::props::TerminalReport {
                rows,
                cols,
                fullscreen,
                focused,
            },
        );
        // 埋点:全屏切换(fullscreen_changes;仅在相对前态翻转时,滤掉每 tick 的等值上报)。
        if let Some(fullscreen) = toggled {
            self.record_behavior(mineral_stats::BehaviorEvent::FullscreenChange { fullscreen });
        }
    }
}

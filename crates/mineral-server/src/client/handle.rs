//! [`ClientHandle`]:daemon 内部业务调用面。

use mineral_channel_core::ChannelCaps;
use mineral_model::{Song, SongId, SourceKind};
use mineral_protocol::{
    DownloadId, DownloadTarget, Event, PlayMode, PlayQueueError, PlayerSync, PlayerVersions,
    QueueContextWire, QueueEditOutcome, QueueOp, SongStatsWire,
};
use mineral_task::{Priority, TaskEvent, TaskKind};

use super::wire::{edited_song_id, queue_context_from_wire, stats_queue_op};
use crate::player::PlayerCore;

/// 同进程 client handle:持 `PlayerCore` 的 Arc 句柄,所有调用直接 forward。
/// `Clone` 廉价。
#[derive(Clone)]
pub struct ClientHandle {
    /// Player 业务核心(队列/播放模式/任务调度)。
    player: PlayerCore,

    /// 本 handle 归属的连接 id:wire 接入经 [`Self::for_connection`] 每连接
    /// 唯一,per-conn 状态(终端上报)以它归属。
    conn: u64,
}

impl ClientHandle {
    /// 同进程构造,Server 启动后用持有的 `player` 直接拼成 handle。
    pub(crate) fn new(player: PlayerCore) -> Self {
        Self { player, conn: 0 }
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
            return self.queue_edit(&op);
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
    /// `terminal` 属性回 None)。
    pub(crate) fn connection_closed(&self) {
        self.player.clear_terminal_state(self.conn);
    }

    /// 拉取脚本 bind 表(serve 层处理 `ScriptBinds` 用);无脚本 / 线程退出为空。
    pub(crate) async fn script_binds_async(&self) -> Vec<mineral_protocol::ScriptBind> {
        let Some(script) = self.player.script_sender() else {
            return Vec::new();
        };
        script.script_binds().await.unwrap_or_default()
    }

    /// 查询一首歌的本地播放统计，转成 protocol DTO。
    ///
    /// 当前不采集该来源(`stats.level = off` / `exclude_sources` / stats.db 降级)时返回
    /// `None`，即使磁盘留有历史数据也不展示；启用采集但从未播放则返回零值。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲 id；其 namespace 决定采集策略与 persist scope
    ///
    /// # Return:
    ///   当前采集可用时返回 [`mineral_protocol::SongStatsWire`]，否则返回 `None`
    pub(crate) async fn query_song_stats_async(
        &self,
        id: &SongId,
    ) -> color_eyre::Result<Option<mineral_protocol::SongStatsWire>> {
        if !self.player.inner.stats.records_plays_for(id.namespace()) {
            return Ok(None);
        }
        // complete/skip/listen/last 由 stats.db 聚合(全量窗口)；loved 是功能状态，仍读
        // mineral.db。启用采集但没有事实行时构造零值，让 0 与未采集的 None 保持可区分。
        let summary = self.player.inner.stats.store().song_summary(id).await?;
        let loved = self
            .player
            .persist()
            .scope(id.namespace())
            .is_loved(id)
            .await?;

        Ok(Some(match summary {
            Some(s) => SongStatsWire {
                play_count: u32::try_from(s.completed).unwrap_or(u32::MAX),
                skip_count: u32::try_from(s.skips).unwrap_or(u32::MAX),
                total_listen_ms: u64::try_from(s.listen_ms).unwrap_or(0),
                last_played_at: s.last_played_at,
                loved,
            },
            None => SongStatsWire {
                play_count: 0,
                skip_count: 0,
                total_listen_ms: 0,
                last_played_at: None,
                loved,
            },
        }))
    }
}

impl ClientHandle {
    /// 暂停播放。
    pub(crate) fn pause(&self) {
        // 传输面(执行 + 埋点)统一走 PlayerCore 的 transport 方法,client 只穿透 actor。
        self.player.pause_playback(mineral_stats::Actor::User);
    }

    /// 从暂停恢复。
    pub(crate) fn resume(&self) {
        self.player.resume_playback(mineral_stats::Actor::User);
    }

    /// 停止当前曲目。
    pub(crate) fn stop(&self) {
        self.player.stop_playback();
    }

    /// 跳到绝对位置(ms)。
    pub(crate) fn seek(&self, position_ms: u64) {
        self.player
            .seek_playback(position_ms, mineral_stats::Actor::User);
    }

    /// 设置音量百分比(0..=100)。
    pub(crate) fn set_volume(&self, pct: u8) {
        self.player
            .set_playback_volume(pct, mineral_stats::Actor::User);
    }

    /// 直接播放一首歌。
    pub(crate) fn play_song(&self, song: &Song) {
        // 直接改播会顶掉在播曲:先按 skip 结算它(next/prev/EOF 各有自己的结算,唯独这
        // 条显式点播路径需要在此结算,否则被打断曲的 plays 行丢失)。
        self.player.settle_interrupted();
        self.player.play_song(
            song,
            mineral_stats::PlayOrigin::Explicit,
            mineral_stats::Actor::User,
        );
    }

    /// 原子替换 queue 并起播 request-local target occurrence。
    ///
    /// # Params:
    ///   - `songs`: 新队列
    ///   - `target`: `songs` 内的 0-based queue index
    ///   - `context`: 队列语境(埋点 provenance)
    ///
    /// # Return:
    ///   成功起播返回 `Ok(())`;invalid queue 返回 structured error,状态不变。
    pub(crate) fn play_queue(
        &self,
        songs: Vec<Song>,
        target: usize,
        context: QueueContextWire,
    ) -> Result<(), PlayQueueError> {
        let count = i64::try_from(songs.len()).unwrap_or(i64::MAX);
        let song = self
            .player
            .replace_queue(songs, target, queue_context_from_wire(context))?;
        self.player.settle_interrupted();
        self.player.play_song(
            &song,
            mineral_stats::PlayOrigin::Explicit,
            mineral_stats::Actor::User,
        );
        self.record_behavior(mineral_stats::BehaviorEvent::QueueOp {
            op: mineral_stats::QueueOp::Set,
            song: None,
            count,
        });
        Ok(())
    }

    /// 保序插播整组歌曲，实际入队后只记一条操作统计；单首记录歌曲 ID，多首只记录数量。
    ///
    /// # Params:
    ///   - `songs`: 本次插播的整组歌曲，保留重复项
    ///   - `context`: 整组的来源语境
    pub(crate) fn queue_insert_next(&self, songs: Vec<Song>, context: QueueContextWire) {
        let count = songs.len();
        let song = match songs.as_slice() {
            [song] => Some(song.id.clone()),
            _ => None,
        };
        if self
            .player
            .queue_insert_next(songs, queue_context_from_wire(context))
        {
            self.record_behavior(mineral_stats::BehaviorEvent::QueueOp {
                op: mineral_stats::QueueOp::InsertNext,
                song,
                count: i64::try_from(count).unwrap_or(i64::MAX),
            });
        }
    }

    /// 保序追加整组歌曲，实际入队后只记一条操作统计；单首记录歌曲 ID，多首只记录数量。
    ///
    /// # Params:
    ///   - `songs`: 本次追加的整组歌曲，保留重复项
    ///   - `context`: 整组的来源语境
    pub(crate) fn queue_append(&self, songs: Vec<Song>, context: QueueContextWire) {
        let count = songs.len();
        let song = match songs.as_slice() {
            [song] => Some(song.id.clone()),
            _ => None,
        };
        if self
            .player
            .queue_append(songs, queue_context_from_wire(context))
        {
            self.record_behavior(mineral_stats::BehaviorEvent::QueueOp {
                op: mineral_stats::QueueOp::Append,
                song,
                count: i64::try_from(count).unwrap_or(i64::MAX),
            });
        }
    }

    /// 队列结构编辑:删除 / 重排 / 批量清理 / 撤销。
    ///
    /// # Params:
    ///   - `op`: 待执行的操作
    ///
    /// # Return:
    ///   本次编辑的结果。
    pub(crate) fn queue_edit(&self, op: &QueueOp) -> QueueEditOutcome {
        let before = self.player.with_state(|st| st.queue.len());
        let outcome = self.player.queue_edit(op);
        if matches!(outcome, QueueEditOutcome::Applied) {
            let after = self.player.with_state(|st| st.queue.len());
            self.record_behavior(mineral_stats::BehaviorEvent::QueueOp {
                op: stats_queue_op(op),
                song: edited_song_id(op),
                // 纯重排不改长度,记 1 条「受影响」;批量清理记实际删除条数。
                count: i64::try_from(before.abs_diff(after).max(1)).unwrap_or(i64::MAX),
            });
        }
        outcome
    }

    /// 全部已注册 channel 的能力声明。
    pub(crate) fn channel_caps(&self) -> Vec<(SourceKind, ChannelCaps)> {
        self.player.channel_caps()
    }

    /// `m` 键:cycle PlayMode。
    pub(crate) fn cycle_play_mode(&self) {
        // mode_changes 埋点在 PlayerCore 单点(cycle / 直设 / 脚本共用)。
        self.player.cycle_play_mode(mineral_stats::Actor::User);
    }

    /// 直接设置播放模式。
    ///
    /// # Params:
    ///   - `mode`: 目标模式
    pub(crate) fn set_play_mode(&self, mode: PlayMode) {
        // 埋点同 `cycle_play_mode`:PlayerCore 单点、同档不记。
        self.player.set_play_mode(mode, mineral_stats::Actor::User);
    }

    /// `p` 键:进度 > 阈值时回开头,否则跳上一首。
    pub(crate) fn prev_or_restart(&self) {
        self.player.prev_or_restart(mineral_stats::Actor::User);
    }

    /// `n` 键:按 PlayMode 切下一首。
    pub(crate) fn next_song(&self) {
        self.player.next_song(mineral_stats::Actor::User);
    }

    /// 版本门控的播放状态同步。
    ///
    /// # Params:
    ///   - `known`: client 已持有的版本号(0 = 一无所有)
    pub(crate) fn player_sync(&self, known: PlayerVersions) -> PlayerSync {
        self.player.sync(known)
    }

    /// 播放状态变更订阅端。
    pub(crate) fn state_changes(&self) -> tokio::sync::watch::Receiver<u64> {
        self.player.state_changes()
    }

    /// 提交一个任务。
    ///
    /// # Params:
    ///   - `kind`: 任务类型
    ///   - `priority`: 优先级
    pub(crate) fn submit_task(&self, kind: TaskKind, priority: Priority) {
        self.player.submit_task(kind, priority);
    }

    /// 提交单曲 / 歌单下载。
    ///
    /// # Params:
    ///   - `target`: 下载目标
    pub(crate) fn download(&self, target: DownloadTarget) {
        self.player.download(target)
    }

    /// Stop 一个下载。
    ///
    /// # Params:
    ///   - `id`: 下载 id
    pub(crate) fn stop_download(&self, id: &DownloadId) -> color_eyre::Result<()> {
        self.player.stop_download(id)
    }

    /// 上报终端 UI 状态。
    ///
    /// # Params:
    ///   - `rows`: 终端行数
    ///   - `cols`: 终端列数
    ///   - `fullscreen`: 是否全屏播放态
    ///   - `focused`: 终端是否持有焦点
    pub(crate) fn report_terminal_state(
        &self,
        rows: u16,
        cols: u16,
        fullscreen: bool,
        focused: bool,
    ) {
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

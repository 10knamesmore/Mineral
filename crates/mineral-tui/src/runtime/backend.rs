//! TUI 后端端口:生产实现是 [`mineral_client::Client`] 会话,测试实现是进程内假后端。
//!
//! 端口只做两件事:
//! - **读镜像**:共享状态从本地订阅镜像读取,绘制与输入处理无需等待 IPC 应答。
//! - **提交操作**:操作立即入队并返回;结论经 [`CompletionQueue`] 异步回流,
//!   App 每帧 drain,失败按结构化类别提示。
//!
//! 这是 UI 与 client 的接缝(测试替身也走同一接口),不是 daemon 侧的业务契约:
//! 生产路径唯一实现是 [`ClientBackend`]。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, Sender};

use mineral_channel_core::ChannelCaps;
use mineral_model::{Song, SongId, SourceKind};
use mineral_protocol::{
    CopyTemplateCtx, DownloadId, DownloadTarget, Event, KeyContext, QueueContextWire,
    QueueEditOutcome, QueueOp, Request, ScriptBind, SubscriptionTopic,
};
use mineral_task::{Priority, Snapshot, TaskKind};

use mineral_client::Client;
use mineral_client::operation::{Outcome, Pending, SubmitError};
use mineral_client::state::{
    DownloadsDetailMirror, PlaybackMirror, PlayerMirror, WindowTitleOverride,
};

/// 操作完成事件(daemon 结论回流到 UI)。
#[derive(Debug)]
pub(crate) enum Completion {
    /// 队列原子替换的结果(失败时提示)。
    PlayQueue(Outcome<()>),

    /// 队列编辑回执。
    QueueEdit(Outcome<QueueEditOutcome>),

    /// 脚本动作执行结果。
    ScriptAction {
        /// 动作注册名(提示用)。
        name: String,

        /// 结论。
        outcome: Outcome<()>,
    },

    /// 复制模板渲染结果。
    CopyTemplate(Outcome<Result<String, String>>),

    /// 喜欢切换结果(乐观值以服务端结论校正)。
    Love {
        /// 目标歌曲(提示 / 校正归属用)。
        song_id: SongId,

        /// 切换后的权威状态。
        outcome: Outcome<bool>,
    },

    /// 下载 Stop 结果。
    StopDownload(Outcome<()>),

    /// 脚本绑定表刷新(重新拉取完成)。
    ScriptBinds(Vec<ScriptBind>),
}

/// 完成事件队列:后端写入、App 每帧 drain(不阻塞 UI 线程)。
pub(crate) struct CompletionQueue {
    /// 发送端,供后台任务并发投递。
    tx: Sender<Completion>,

    /// 接收端(App 独占 drain)。
    rx: Mutex<Receiver<Completion>>,
}

impl Default for CompletionQueue {
    /// 默认队列自带收发两端(测试替身直接 `default()` 也能回流完成事件)。
    fn default() -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        Self {
            tx,
            rx: Mutex::new(rx),
        }
    }
}

impl CompletionQueue {
    /// 建一个自含收发两端的队列。
    #[must_use]
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// 投递一条完成事件,由 App 后续取走。
    ///
    /// # Params:
    ///   - `completion`: 完成事件
    pub(crate) fn push(&self, completion: Completion) {
        let _ = self.tx.send(completion);
    }

    /// 取走当前全部完成事件。
    #[must_use]
    pub(crate) fn drain(&self) -> Vec<Completion> {
        let mut out = Vec::new();
        if let Ok(rx) = self.rx.lock() {
            while let Ok(completion) = rx.try_recv() {
                out.push(completion);
            }
        }
        out
    }
}

/// 启动自举数据(连接后一次拉齐,UI 构造期读取)。
#[derive(Clone, Debug, Default)]
pub(crate) struct BackendBootstrap {
    /// 各源能力声明。
    pub(crate) channel_caps: Vec<(SourceKind, ChannelCaps)>,

    /// 脚本键绑定表。
    pub(crate) script_binds: Vec<ScriptBind>,
}

/// TUI 后端端口。
pub(crate) trait Backend: Send + Sync {
    /// 启动自举数据(能力表 / 脚本绑定)。
    fn bootstrap(&self) -> BackendBootstrap;

    /// 重新拉取脚本绑定表(脚本热重载后调;结果经完成事件回流)。
    fn refresh_script_binds(&self);

    /// 完成事件队列。
    fn completions(&self) -> &Arc<CompletionQueue>;

    /// 链路是否可用。
    fn connected(&self) -> bool;

    /// 取走待消费事件。
    fn drain_events(&self) -> Vec<Event>;

    /// 因容量丢弃的事件数(诊断用)。
    fn events_dropped(&self) -> u64;

    /// 只读访问播放镜像。
    ///
    /// # Params:
    ///   - `f`: 读取闭包
    fn with_player(&self, f: &mut dyn FnMut(&PlayerMirror));

    /// 只读访问播放锚点镜像。
    ///
    /// # Params:
    ///   - `f`: 读取闭包
    fn with_playback(&self, f: &mut dyn FnMut(&PlaybackMirror));

    /// 任务摘要;尚未就绪为 `None`。
    fn tasks(&self) -> Option<Snapshot>;

    /// 下载摘要。
    fn downloads_summary(&self) -> mineral_protocol::DownloadSummary;

    /// 下载明细(未订阅 / 未就绪为 `None`)。
    fn downloads_detail(&self) -> Option<DownloadsDetailMirror>;

    /// 窗口标题覆盖状态。
    fn window_title_override(&self) -> WindowTitleOverride;

    /// 订阅主题(引用计数)。
    ///
    /// # Params:
    ///   - `topic`: 订阅主题
    fn subscribe(&self, topic: SubscriptionTopic);

    /// 退订主题。
    ///
    /// # Params:
    ///   - `topic`: 订阅主题
    fn unsubscribe(&self, topic: SubscriptionTopic);

    /// 取走 PCM 近期窗口样本。
    fn drain_pcm(&self) -> Vec<f32>;

    /// 取走 PCM 断续标记(换代 / 缺口)。
    fn take_pcm_discontinuity(&self) -> bool;

    /// 暂停。
    fn pause(&self);

    /// 恢复播放。
    fn resume(&self);

    /// 跳转到绝对位置(ms)。
    ///
    /// # Params:
    ///   - `position_ms`: 目标位置
    fn seek(&self, position_ms: u64);

    /// 设置音量百分比。
    ///
    /// # Params:
    ///   - `pct`: 0..=100
    fn set_volume(&self, pct: u8);

    /// 循环播放模式。
    fn cycle_play_mode(&self);

    /// 上一首 / 回开头。
    fn prev_or_restart(&self);

    /// 下一首。
    fn next_song(&self);

    /// 直接播放一首歌。
    ///
    /// # Params:
    ///   - `song`: 目标歌曲
    fn play_song(&self, song: Song);

    /// 原子替换队列并起播。
    ///
    /// # Params:
    ///   - `songs`: 新队列
    ///   - `target`: 起播下标
    ///   - `context`: 队列语境
    fn play_queue(&self, songs: Vec<Song>, target: usize, context: QueueContextWire);

    /// 插播到当前曲之后。
    ///
    /// # Params:
    ///   - `song`: 待插播歌曲
    ///   - `context`: 来源语境
    fn queue_insert_next(&self, song: Song, context: QueueContextWire);

    /// 追加到队列末尾。
    ///
    /// # Params:
    ///   - `song`: 待追加歌曲
    ///   - `context`: 来源语境
    fn queue_append(&self, song: Song, context: QueueContextWire);

    /// 队列结构编辑(结论经完成事件回流)。
    ///
    /// # Params:
    ///   - `op`: 编辑操作
    fn queue_edit(&self, op: QueueOp);

    /// 提交 scheduler 任务。
    ///
    /// # Params:
    ///   - `kind`: 任务类型
    ///   - `priority`: 优先级
    fn submit_task(&self, kind: TaskKind, priority: Priority);

    /// 提交下载。
    ///
    /// # Params:
    ///   - `target`: 下载目标
    fn download(&self, target: DownloadTarget);

    /// Stop 一个下载。
    ///
    /// # Params:
    ///   - `id`: 下载 id
    fn stop_download(&self, id: DownloadId);

    /// 切换喜欢状态。
    ///
    /// # Params:
    ///   - `song`: 目标歌曲
    fn toggle_love(&self, song: Song);

    /// 后台查询本地播放统计(结果经事件流回流)。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲
    fn request_song_stats(&self, id: SongId);

    /// 触发脚本动作。
    ///
    /// # Params:
    ///   - `name`: 动作名
    ///   - `ctx`: 按键上下文
    fn invoke_action(&self, name: &str, ctx: Option<KeyContext>);

    /// 渲染复制模板。
    ///
    /// # Params:
    ///   - `index`: 模板下标
    ///   - `ctx`: 模板实体
    fn render_copy_template(&self, index: usize, ctx: CopyTemplateCtx);

    /// 上报终端 UI 状态。
    ///
    /// # Params:
    ///   - `rows`: 终端行数
    ///   - `cols`: 终端列数
    ///   - `fullscreen`: 是否全屏播放态
    ///   - `focused`: 终端是否持有焦点
    fn report_terminal_state(&self, rows: u16, cols: u16, fullscreen: bool, focused: bool);

    /// 请求 daemon 优雅退出。
    fn request_daemon_shutdown(&self);
}

/// 生产实现:`mineral-client` 会话 + 自举缓存。
pub(crate) struct ClientBackend {
    /// 会话 client。
    client: Arc<Client>,

    /// 启动自举数据。
    bootstrap: BackendBootstrap,

    /// 完成事件队列。
    completions: Arc<CompletionQueue>,
}

impl ClientBackend {
    /// 组装生产后端。
    ///
    /// # Params:
    ///   - `client`: 已连接的会话 client
    ///   - `bootstrap`: 启动自举数据
    ///   - `completions`: 完成事件队列
    #[must_use]
    pub(crate) fn new(
        client: Arc<Client>,
        bootstrap: BackendBootstrap,
        completions: Arc<CompletionQueue>,
    ) -> Self {
        Self {
            client,
            bootstrap,
            completions,
        }
    }

    /// 把一条 pending 转成完成事件(在 tokio 任务里等待)。
    fn spawn_pending<T, F>(&self, pending: Result<Pending<T>, SubmitError>, wrap: F)
    where
        T: Send + 'static,
        F: FnOnce(Outcome<T>) -> Completion + Send + 'static,
    {
        let queue = Arc::clone(&self.completions);
        match pending {
            Ok(pending) => {
                if let Ok(handle) = tokio::runtime::Handle::try_current() {
                    handle.spawn(async move {
                        queue.push(wrap(pending.outcome().await));
                    });
                } else {
                    // 没有 runtime 就无法等待结果;丢弃结果句柄并记录警告。
                    mineral_log::warn!(target: "tui", "无 tokio runtime,操作结论无法回流");
                }
            }
            Err(error) => {
                queue.push(wrap(Outcome::Unknown {
                    detail: format!("未提交:{error}"),
                }));
            }
        }
    }
}

impl Backend for ClientBackend {
    fn bootstrap(&self) -> BackendBootstrap {
        self.bootstrap.clone()
    }

    fn refresh_script_binds(&self) {
        let queue = Arc::clone(&self.completions);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let client = Arc::clone(&self.client);
            handle.spawn(async move {
                if let Some(binds) = client.script_binds().await.into_success() {
                    queue.push(Completion::ScriptBinds(binds));
                }
            });
        }
    }

    fn completions(&self) -> &Arc<CompletionQueue> {
        &self.completions
    }

    fn connected(&self) -> bool {
        self.client.connected()
    }

    fn drain_events(&self) -> Vec<Event> {
        self.client.mirror().drain_events()
    }

    fn events_dropped(&self) -> u64 {
        self.client.mirror().events_dropped()
    }

    fn with_player(&self, f: &mut dyn FnMut(&PlayerMirror)) {
        self.client.mirror().read_player(|player| f(player));
    }

    fn with_playback(&self, f: &mut dyn FnMut(&PlaybackMirror)) {
        self.client.mirror().read_playback(|playback| f(playback));
    }

    fn tasks(&self) -> Option<Snapshot> {
        self.client.tasks_snapshot()
    }

    fn downloads_summary(&self) -> mineral_protocol::DownloadSummary {
        self.client
            .mirror()
            .read_downloads_summary(mineral_protocol::DownloadSummary::clone)
    }

    fn downloads_detail(&self) -> Option<DownloadsDetailMirror> {
        self.client.mirror().downloads_detail_snapshot()
    }

    fn window_title_override(&self) -> WindowTitleOverride {
        self.client.mirror().window_title_override()
    }

    fn subscribe(&self, topic: SubscriptionTopic) {
        let _ = self.client.subscribe(topic);
    }

    fn unsubscribe(&self, topic: SubscriptionTopic) {
        self.client.unsubscribe(topic);
    }

    fn drain_pcm(&self) -> Vec<f32> {
        self.client.drain_pcm()
    }

    fn take_pcm_discontinuity(&self) -> bool {
        self.client.mirror().take_pcm_discontinuity()
    }

    fn pause(&self) {
        self.client.fire(Request::Pause);
    }

    fn resume(&self) {
        self.client.fire(Request::Resume);
    }

    fn seek(&self, position_ms: u64) {
        self.client.fire(Request::Seek(position_ms));
    }

    fn set_volume(&self, pct: u8) {
        self.client.fire(Request::SetVolume(pct));
    }

    fn cycle_play_mode(&self) {
        self.client.fire(Request::CyclePlayMode);
    }

    fn prev_or_restart(&self) {
        self.client.fire(Request::PrevOrRestart);
    }

    fn next_song(&self) {
        self.client.fire(Request::NextSong);
    }

    fn play_song(&self, song: Song) {
        self.client.fire(Request::PlaySong(Box::new(song)));
    }

    fn play_queue(&self, songs: Vec<Song>, target: usize, context: QueueContextWire) {
        self.spawn_pending(
            self.client.play_queue(songs, target, context),
            Completion::PlayQueue,
        );
    }

    fn queue_insert_next(&self, song: Song, context: QueueContextWire) {
        self.client.fire(Request::QueueInsertNext {
            song: Box::new(song),
            context,
        });
    }

    fn queue_append(&self, song: Song, context: QueueContextWire) {
        self.client.fire(Request::QueueAppend {
            song: Box::new(song),
            context,
        });
    }

    fn queue_edit(&self, op: QueueOp) {
        self.spawn_pending(self.client.queue_edit_pending(op), Completion::QueueEdit);
    }

    fn submit_task(&self, kind: TaskKind, priority: Priority) {
        self.client.fire(Request::SubmitTask(kind, priority));
    }

    fn download(&self, target: DownloadTarget) {
        self.client.fire(Request::Download(target));
    }

    fn stop_download(&self, id: DownloadId) {
        self.spawn_pending(
            self.client.stop_download_pending(id),
            Completion::StopDownload,
        );
    }

    fn toggle_love(&self, song: Song) {
        let song_id = song.id.clone();
        self.spawn_pending(self.client.toggle_love(song), move |outcome| {
            Completion::Love { song_id, outcome }
        });
    }

    fn request_song_stats(&self, id: SongId) {
        self.client.request_song_stats(id);
    }

    fn invoke_action(&self, name: &str, ctx: Option<KeyContext>) {
        let name = name.to_owned();
        let pending = self.client.invoke_action_pending(&name, ctx, Vec::new());
        self.spawn_pending(pending, move |outcome| Completion::ScriptAction {
            name,
            outcome,
        });
    }

    fn render_copy_template(&self, index: usize, ctx: CopyTemplateCtx) {
        self.spawn_pending(
            self.client.render_copy_template_pending(index, ctx),
            Completion::CopyTemplate,
        );
    }

    fn report_terminal_state(&self, rows: u16, cols: u16, fullscreen: bool, focused: bool) {
        self.client.fire(Request::TerminalState {
            rows,
            cols,
            fullscreen,
            focused,
        });
    }

    fn request_daemon_shutdown(&self) {
        self.client.fire(Request::Shutdown);
    }
}

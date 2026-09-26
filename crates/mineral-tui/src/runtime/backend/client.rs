//! 用 mineral-client 会话实现 TUI 后端，统一处理任务提交和异步操作结论。

use std::sync::Arc;

use mineral_client::Client;
use mineral_client::operation::{Outcome, Pending, SubmitError};
use mineral_client::state::{
    DownloadsDetailMirror, PlaybackMirror, PlayerMirror, WindowTitleOverride,
};
use mineral_model::{Song, SongId};
use mineral_protocol::{
    CopyTemplateCtx, DownloadId, DownloadTarget, Event, KeyContext, QueueContextWire, QueueOp,
    Request, SubscriptionTopic,
};
use mineral_task::{Priority, Snapshot, TaskKind};

use super::completion::{Completion, CompletionQueue};
use super::port::{Backend, BackendBootstrap};
use super::task_submissions::TaskSubmissions;

/// 生产实现:`mineral-client` 会话 + 自举缓存。
pub(crate) struct ClientBackend {
    /// 会话 client。
    client: Arc<Client>,

    /// 启动自举数据。
    bootstrap: BackendBootstrap,

    /// 完成事件队列。
    completions: Arc<CompletionQueue>,

    /// 所有任务共用的待提交队列，独立于页面和实体加载状态。
    task_submissions: parking_lot::Mutex<TaskSubmissions>,
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
            task_submissions: parking_lot::Mutex::new(TaskSubmissions::default()),
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
    fn audio_outputs(&self) {
        self.spawn_pending(self.client.audio_outputs(), Completion::AudioOutputs);
    }

    fn set_audio_output(&self, target: mineral_audio::OutputTarget) {
        self.spawn_pending(
            self.client.set_audio_output(target),
            Completion::AudioOutputSelected,
        );
    }

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

    fn queue_insert_next(&self, songs: Vec<Song>, context: QueueContextWire) {
        self.client
            .fire(Request::QueueInsertNext { songs, context });
    }

    fn queue_append(&self, songs: Vec<Song>, context: QueueContextWire) {
        self.client.fire(Request::QueueAppend { songs, context });
    }

    fn queue_edit(&self, op: QueueOp) {
        self.spawn_pending(self.client.queue_edit_pending(op), Completion::QueueEdit);
    }

    fn submit_task(&self, kind: TaskKind, priority: Priority) {
        let mut tasks = self.task_submissions.lock();
        tasks.enqueue(kind, priority);
        if !tasks.is_blocked() {
            tasks.flush(|kind, priority| self.client.try_fire(Request::SubmitTask(kind, priority)));
        }
    }

    fn flush_task_submissions(&self) {
        self.task_submissions
            .lock()
            .flush(|kind, priority| self.client.try_fire(Request::SubmitTask(kind, priority)));
    }

    fn prioritize_task(&self, kind: &TaskKind) {
        self.task_submissions.lock().prioritize(kind);
    }

    fn pending_task_count(&self) -> usize {
        self.task_submissions.lock().pending_count()
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

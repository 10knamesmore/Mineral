//! TUI 后端接口：读取本地镜像，提交操作，并由统一提交层管理待发送任务。

use std::sync::Arc;

use mineral_channel_core::ChannelCaps;
use mineral_client::state::{
    DownloadsDetailMirror, PlaybackMirror, PlayerMirror, WindowTitleOverride,
};
use mineral_model::{Song, SongId, SourceKind};
use mineral_protocol::{
    CopyTemplateCtx, DownloadId, DownloadTarget, Event, KeyContext, QueueContextWire, QueueOp,
    ScriptBind, SubscriptionTopic,
};
use mineral_task::{Priority, Snapshot, TaskKind};

use super::completion::CompletionQueue;

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

    /// 一次提交整组歌曲，按原序插播到当前曲之后。
    ///
    /// # Params:
    ///   - `songs`: 待插播歌曲，保留重复项
    ///   - `context`: 这组歌曲的来源语境
    fn queue_insert_next(&self, songs: Vec<Song>, context: QueueContextWire);

    /// 一次提交整组歌曲，按原序追加到队列末尾。
    ///
    /// # Params:
    ///   - `songs`: 待追加歌曲，保留重复项
    ///   - `context`: 这组歌曲的来源语境
    fn queue_append(&self, songs: Vec<Song>, context: QueueContextWire);

    /// 队列结构编辑(结论经完成事件回流)。
    ///
    /// # Params:
    ///   - `op`: 编辑操作
    fn queue_edit(&self, op: QueueOp);

    /// 接收 scheduler 任务；提交层统一保留容量不足时的任务并在后续 tick 重试。
    ///
    /// # Params:
    ///   - `kind`: 任务类型
    ///   - `priority`: 优先级
    fn submit_task(&self, kind: TaskKind, priority: Priority);

    /// 推进所有待提交任务；不依赖具体页面，已发出的请求不会重发。
    fn flush_task_submissions(&self);

    /// 提升仍未发出的当前关注任务，不重复提交已经发出的任务。
    fn prioritize_task(&self, kind: &TaskKind);

    /// 本地仍在等待提交的后台任务数量。
    fn pending_task_count(&self) -> usize;

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

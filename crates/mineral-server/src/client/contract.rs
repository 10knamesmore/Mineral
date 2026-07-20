//! Client → Server 调用面的抽象契约。

use mineral_audio::AudioSnapshot;
use mineral_channel_core::ChannelCaps;
use mineral_model::{MediaUrl, Song, SongId, SourceKind};
use mineral_protocol::{
    CancelFilter, DownloadProgress, DownloadTarget, PlayerSync, PlayerVersions, QueueContextWire,
    QueueEditOutcome, QueueOp, SongStatsWire,
};
use mineral_task::{Priority, Snapshot, TaskId, TaskKind};

/// Client → Server 调用面的抽象。
///
/// **实现方约定**:全部方法 sync。fire-and-forget 类立即返回;返回值类允许阻塞等
/// 内部 I/O,但调用方期望 < 1ms。出错时返回值类用「合理默认值」兜底。
pub trait Client: Send + Sync {
    // ---- 低级播放控制(给 SetVolume / Pause / Resume / Seek 等无业务语义的) ----
    /// 切到这个 URL,从头播。**通常 client 不应直调** —— 用 [`Self::play_song`] 让
    /// server 跑完整流程;此方法是低级入口,主要给 `mineral status` 等 debug client 用。
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

    /// 队列结构编辑:删除 / 重排 / 批量清理 / 撤销。
    ///
    /// 脚本变换要跨线程执行、必须异步,不走此同步入口。
    ///
    /// # Params:
    ///   - `op`: 待执行的操作
    ///
    /// # Return:
    ///   本次编辑的结果。
    fn queue_edit(&self, op: QueueOp) -> QueueEditOutcome;

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
    /// 当前 scheduler 状态快照。
    fn task_snapshot(&self) -> Snapshot;

    // ---- PCM 流(client spectrum 用) ----
    /// 拉最多 N 个 PCM sample。返回 (samples, sample_rate);可能短于 N。
    fn pull_pcm(&self, n: usize) -> (Vec<f32>, u32);

    // ---- 喜欢 / 统计 ----
    /// 切换一首歌的喜欢(♥)状态,返回切换后的新 loved 态。
    ///
    /// daemon 模式经 IPC 拿到真实结果;in-proc 模式 fire-and-forget,返回值为乐观占位
    /// (调用方应自行乐观更新本地 loved 态,不强依赖此返回值)。
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
    ///   执行失败),调用方应提示用户。
    fn invoke_action(
        &self,
        name: &str,
        ctx: Option<mineral_protocol::KeyContext>,
    ) -> Option<String> {
        let _ = (name, ctx);
        Some("脚本动作不可用(当前 client 不支持)".to_owned())
    }

    /// 拉取脚本 `mineral.bind` 的键绑定表(启动 / 配置重载后调,合进自己的 keymap)。
    ///
    /// 默认空(in-proc 调试模式不起脚本线程,空表即正确语义);daemon 模式经 IPC 拿真表。
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
    ///   `Ok(text)` = 进剪贴板的文本;`Err(msg)` = 人读错误,调用方应提示。
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
    /// 进度 / 完成经 [`mineral_task::TaskEvent::Notice`] 回传。
    ///
    /// # Params:
    ///   - `target`: 下载目标(单曲 / 歌单)
    fn download(&self, target: DownloadTarget);

    /// 拉一次下载进度快照。无下载时 `active == false`。
    fn download_progress(&self) -> DownloadProgress;

    /// 上报终端 UI 状态(resize / 全屏切换时调;值没变调用方应去抖不发)。
    /// daemon 灌属性树 `terminal` 复合属性供脚本 observe。fire-and-forget。
    ///
    /// 默认 no-op(无界面 / 测试实现不上报)。
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
    /// 同进程实现恒 `true`(client 与 server 同生共死)。跨进程实现在 daemon 断开后返回
    /// `false`,调用方据此干净退出而非僵死在「所有请求兜底默认值」的状态。
    fn connected(&self) -> bool {
        true
    }

    /// 请求 daemon 优雅退出。
    ///
    /// 默认 no-op——同进程实现下调用方与 server 同生共死,进程退 = server drop,不存在
    /// 可独立停掉的 daemon。跨进程实现经 IPC 发关停请求;断连时静默失败(调用方已在
    /// 退出路径上,无从补救也无需提示)。
    fn request_daemon_shutdown(&self) {}

    /// 拉走 server 主动推送的 [`mineral_protocol::Event`](握手订阅后 server 随时下推、
    /// 调用方侧缓冲,每 tick drain;任务 / 数据事件也经
    /// [`mineral_protocol::Event::Task`] 走此通道)。
    ///
    /// 跨进程实现返回缓冲的推送;同进程 / 测试实现用默认空(in-proc 无推送通道)。
    fn drain_events(&self) -> Vec<mineral_protocol::Event> {
        Vec::new()
    }
}

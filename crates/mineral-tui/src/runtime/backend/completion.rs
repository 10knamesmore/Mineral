//! 后端操作完成事件及其到 TUI 主循环的交付队列。

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use mineral_client::operation::Outcome;
use mineral_model::SongId;
use mineral_protocol::{QueueEditOutcome, ScriptBind};

/// 操作完成事件(daemon 结论回流到 UI)。
#[derive(Debug)]
pub(crate) enum Completion {
    /// CPAL device list or structured failure.
    AudioOutputs(Outcome<Vec<mineral_audio::OutputDevice>>),

    /// The daemon's stream switch result.
    AudioOutputSelected(Outcome<()>),

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

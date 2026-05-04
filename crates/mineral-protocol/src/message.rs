//! IPC 消息类型 — [`Request`] 与 [`Response`]。
//!
//! 与 [`mineral_server::ClientHandle`] 的方法 1:1 对应;`Response` 的 variant 由
//! 调用方根据自己发的 `Request` 决定预期。错误统一走 [`Response::Error`]。

use mineral_audio::AudioSnapshot;
use mineral_model::MediaUrl;
use mineral_task::{Priority, Snapshot, TaskEvent, TaskId, TaskKind};
use serde::{Deserialize, Serialize};

use crate::CancelFilter;

/// Client → Server 命令。每条 [`Request`] 一定有一条对应的 [`Response`]。
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum Request {
    // ---- 播放控制 ----
    /// 切到指定 URL 播放(对应 [`mineral_server::ClientHandle::play`])。
    Play(MediaUrl),

    /// 暂停。
    Pause,

    /// 从暂停恢复。
    Resume,

    /// 停止当前曲目。
    Stop,

    /// 跳到绝对位置(ms),latest-wins。
    Seek(u64),

    /// 设置音量百分比(0..=100)。
    SetVolume(u8),

    /// 拉一次音频快照。返回 [`Response::AudioSnapshot`]。
    AudioSnapshot,

    // ---- 任务调度 ----
    /// 提交一个任务。返回 [`Response::TaskId`]。
    SubmitTask(TaskKind, Priority),

    /// 按过滤条件批量取消任务。返回 [`Response::Ok`]。
    CancelTasks(CancelFilter),

    /// 拉走 server 端积攒的所有任务事件。返回 [`Response::TaskEvents`]。
    DrainTaskEvents,

    /// 拉一次 scheduler 状态快照。返回 [`Response::TaskSnapshot`]。
    TaskSnapshot,
}

/// Server → Client 应答。
#[derive(Debug, Serialize, Deserialize)]
pub enum Response {
    /// 无返回值的命令成功(play / pause / resume / stop / seek / set_volume / cancel_tasks)。
    Ok,

    /// 对应 [`Request::AudioSnapshot`]。
    AudioSnapshot(AudioSnapshot),

    /// 对应 [`Request::SubmitTask`]。
    TaskId(TaskId),

    /// 对应 [`Request::DrainTaskEvents`]。
    TaskEvents(Vec<TaskEvent>),

    /// 对应 [`Request::TaskSnapshot`]。
    TaskSnapshot(Snapshot),

    /// 服务端处理失败 / 当前不接受新 client / 协议异常。文本人读即可。
    Error(String),
}

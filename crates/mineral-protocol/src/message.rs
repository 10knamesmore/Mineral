//! IPC 消息类型 — [`Request`] 与 [`Response`]。
//!
//! 与 [`mineral_server::ClientHandle`] 的方法 1:1 对应;`Response` 的 variant 由
//! 调用方根据自己发的 `Request` 决定预期。错误统一走 [`Response::Error`]。

use mineral_audio::AudioSnapshot;
use mineral_model::{MediaUrl, Song};
use mineral_task::{Priority, Snapshot, TaskEvent, TaskId, TaskKind};
use serde::{Deserialize, Serialize};

use crate::{CancelFilter, PlayerSnapshot};

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

    // ---- Player 业务 ----(server 持权威 PlayerState)
    /// Client 选了一首歌。Server 内部跑完整 play 流程(cancel 旧 SongUrl/Lyrics、
    /// audio.stop、记录 current_song、命中 prefetched 直接 audio.play、否则
    /// 提交新 SongUrl/Lyrics 任务,等 PlayUrlReady 后内部 audio.play)。
    /// 返回 [`Response::Ok`]。
    /// `Box` 是为了避免 enum 体积膨胀(`Song` 比平均 variant 大很多)。
    PlaySong(Box<Song>),

    /// 替换 queue + 设当前位置。Shuffle 模式下 server 端洗牌。
    /// 返回 [`Response::Ok`]。
    SetQueue {
        /// 新 queue。
        queue: Vec<Song>,
        /// queue 中作为「当前」的歌 id;server 据此设 queue_sel。
        target_id: mineral_model::SongId,
    },

    /// `m` 键循环 PlayMode。返回 [`Response::Ok`]。
    CyclePlayMode,

    /// `p` 键:进度 > 阈值时回开头,否则跳上一首。返回 [`Response::Ok`]。
    PrevOrRestart,

    /// `n` 键:按当前 mode 切下一首。返回 [`Response::Ok`]。
    NextSong,

    /// 拉一份 PlayerSnapshot;client 启动 / 重连时灌进 UI。
    /// 返回 [`Response::PlayerSnapshot`]。
    PlayerSnapshot,

    // ---- PCM 流 ----
    /// 拉最多 N 个 f32 PCM 样本(单声道,FFT 输入用)。
    /// 返回 [`Response::PcmData`]。
    PullPcm(usize),
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

    /// 对应 [`Request::PlayerSnapshot`]。`Box` 避免 enum 体积膨胀。
    PlayerSnapshot(Box<PlayerSnapshot>),

    /// 对应 [`Request::PullPcm`]。
    PcmData {
        /// 0..=N 个样本(可能短于 caller 请求的 N;0 = 当前没数据)。
        samples: Vec<f32>,
        /// 当前 audio 采样率(Hz);0 = 还没在播。client 用它驱动 fft。
        sample_rate: u32,
    },

    /// 服务端处理失败 / 当前不接受新 client / 协议异常。文本人读即可。
    Error(String),
}

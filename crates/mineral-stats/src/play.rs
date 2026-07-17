//! `PlayRecord`:一次播放的完整事实(plays 一行)。

use mineral_model::{AudioFormat, BitRate, SongId};

use crate::context::QueueContext;
use crate::vocab::{Actor, FinishReason, PlayMode, PlayOrigin, PlaybackOrigin};

/// 播放音频快照(plays 行的音频列组:随 play_url 就绪 / 改写整组更新)。
///
/// 起播时未知(全 `None` + 未顶换),play_url 就绪后经 enrich 整组补齐;脚本改写流
/// 再次 enrich 覆盖(此时 `substituted` 为 `true`)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayAudioSnapshot {
    /// 容器 / 编码格式;`None` = 未提供 / 探不出。
    pub audio_format: Option<AudioFormat>,

    /// 实际码率 bps;`None` = 未知。
    pub bitrate_bps: Option<i64>,

    /// 归一化音质档;`None` = 未知。
    pub quality: Option<BitRate>,

    /// 位深(仅本地无损可得);`None` = 未知。
    pub bit_depth: Option<i64>,

    /// 播放 URL 是否被插件顶换。
    pub substituted: bool,
}

/// 一次播放的完整事实。
///
/// 起播时组装大部分字段(音频快照来自 PlayUrl,origin / context / mode 来自队列状态),
/// 结束时补 `ended_at` / `listen_ms` / `finish_reason` / `skip_at_ms` 后一次写齐,不做
/// 行内 UPDATE。领域记录(非配置 struct),server 逐字段装配,故字段公开、可字面量构造。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayRecord {
    /// 歌曲 ID(拆成 `ns` + `song_value` 两列落库)。
    pub song_id: SongId,

    /// 起播时刻(epoch ms)。
    pub started_at: i64,

    /// 结束时刻(epoch ms)。
    pub ended_at: i64,

    /// 实际收听时长 ms(`position_ms` 口径)。
    pub listen_ms: i64,

    /// 播放当时歌曲总长快照 ms;fMP4 等探不出为 `None`。
    pub duration_ms_snapshot: Option<i64>,

    /// 结束原因。
    pub finish_reason: FinishReason,

    /// 跳歌发生位置 ms;非 skip 为 `None`。
    pub skip_at_ms: Option<i64>,

    /// 播放当时模式。
    pub play_mode: PlayMode,

    /// 归属收听会话 id(须已存在于 sessions 表)。
    pub session_id: i64,

    /// 本行怎么发起。
    pub origin: PlayOrigin,

    /// 发起方(人 / 脚本 / 系统 / cli)。
    pub actor: Actor,

    /// 队列上下文(拆成 `context_kind` + `context_ref` 两列落库)。
    pub context: QueueContext,

    /// 音频快照(拆成 audio_format / bitrate_bps / quality / bit_depth / substituted
    /// 五列落库)。
    pub audio: PlayAudioSnapshot,

    /// 音频本体来源位置(下载库 / 缓存 / 远端)。
    pub playback_origin: PlaybackOrigin,
}

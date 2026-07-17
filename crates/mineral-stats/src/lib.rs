//! 行为埋点落库层。
//!
//! 独立 `stats.db`(与 mineral.db 并列、独立迁移链),把播放 / 会话 / 全谱交互记成
//! 只追加的原始事实流水,供跨源长周期聚合(类年终盘点)。写入侧是唯一 writer 的
//! 职责,查询侧对时间窗做跨源聚合。库内无 JSON:每类事件一张强 schema 专表。

mod context;
mod event;
#[cfg(any(test, feature = "fixture"))]
pub mod fixture;
mod params;
mod play;
mod report;
mod session;
mod store;
mod vocab;

pub use context::QueueContext;
pub use event::{
    ActionTrigger, AudioBackend, BehaviorEvent, CacheHarvestOutcome, CopyContext, DownloadHook,
    DownloadOutcome, FailOpen, FetchKind, FetchOutcome, FetchTrigger, GaplessResult, HookDecision,
    HookKind, HookStage, LifecyclePhase, LifecycleWho, LoveOrigin, OpOutcome, PauseAction,
    PlaylistError, PlaylistOpKind, PlaylistRef, PrefetchResolution, PrefetchSource, QueueOp,
    RejectReason, RemoteMirror, ScriptEvent, SearchOutcome, SearchTargetKind, SpawnOutcome,
    StatsEvent, StoreOp, SystemEvent, UrlOutcome, query_hash,
};
pub use params::{Level, Retention, SearchQueryMode, StatsParams};
pub use play::{PlayAudioSnapshot, PlayRecord};
pub use report::{
    Bucket, BucketBy, ContextSlice, Discoveries, Distributions, Endurance, EventCount,
    EventSummary, NamedEntry, PlayTail, RawReport, ReportOptions, Slice, SongSummary, StatsReport,
    StatusReport, Tally, TopAlbum, TopArtist, TopBy, TopSong, Totals, combine,
};
pub use session::{SessionDecision, SessionTracker};
pub use store::{StatsStore, is_event_kind};
pub use vocab::{Actor, FinishReason, PlayMode, PlayOrigin, PlaybackOrigin};

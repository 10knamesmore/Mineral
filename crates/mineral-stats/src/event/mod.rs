//! 埋点事件类型层:顶层封装 + 行为域 / 系统域两组具名事件与其受控词汇枚举。

mod behavior;
mod stats_event;
mod system;

pub use behavior::{
    ActionTrigger, AudioBackend, BehaviorEvent, CopyContext, DownloadHook, DownloadOutcome,
    FetchKind, FetchOutcome, FetchTrigger, LifecyclePhase, LifecycleWho, LoveOrigin, OpOutcome,
    PauseAction, PlaylistError, PlaylistOpKind, PlaylistRef, QueueOp, RejectReason, RemoteMirror,
    SearchOutcome, SearchTargetKind, SpawnOutcome, StoreOp, query_hash,
};
pub use stats_event::StatsEvent;
pub use system::{
    CacheHarvestOutcome, FailOpen, GaplessResult, HookDecision, HookKind, HookStage,
    PrefetchResolution, PrefetchSource, ScriptEvent, SystemEvent, UrlOutcome,
};

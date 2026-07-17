//! 系统域事件:daemon 自治链路(取链 / 插件 / 无缝 / 预取 / 缓存 / 脚本)的流水。
//!
//! 与行为域的区别是**无 actor**——这些事件不由某个人 / 脚本当场发起,而是播放核心
//! 自身的链路副作用。每个变体一一对应一张系统域事件表,具名字段 = 该表除公共列
//! (id / ts / session_id)外的专有列。

use mineral_model::SongId;

/// 取链结局(url_resolutions.outcome)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum UrlOutcome {
    /// 拿到可播 URL。
    Ok,

    /// 后端返回空(无可播源,区别于报错)。
    Empty,

    /// 取链报错。
    Error,
}

/// 触发的插件 hook(hook_fires.hook)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum HookKind {
    /// 起播前 hook。
    BeforeStream,

    /// 下载前 hook。
    BeforeDownload,
}

/// hook 触发的时机(hook_fires.stage)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum HookStage {
    /// 即时(当前曲）。
    Immediate,

    /// 预取(下一曲）。
    Prefetch,
}

/// hook 的裁决(hook_fires.decision)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum HookDecision {
    /// 放行不改。
    Continue,

    /// 改写 URL。
    Rewrite,

    /// 顶掉本次。
    Skip,
}

/// hook 失败时的 fail-open 原因(hook_fires.fail_open);正常裁决为 `None`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum FailOpen {
    /// 超时。
    Timeout,

    /// 执行线程已死。
    ThreadDead,

    /// 执行报错。
    Error,
}

/// 无缝衔接的边界裁决(gapless_boundaries.result)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum GaplessResult {
    /// 采纳无缝边界。
    Adopt,

    /// 回落到普通切歌。
    Fallback,
}

/// 预取的来源(prefetches.source)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum PrefetchSource {
    /// 本地文件。
    Local,

    /// 远端流。
    Remote,

    /// 单曲循环(复用当前）。
    RepeatOne,
}

/// 预取的裁决(prefetches.resolution)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum PrefetchResolution {
    /// 已装填待播。
    Armed,

    /// 被插件否决。
    Vetoed,

    /// 被插件改写。
    Rewritten,

    /// 预取失败。
    Failed,
}

/// 边播边收割的落库结局(cache_harvests.outcome)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum CacheHarvestOutcome {
    /// 入缓存。
    Cached,

    /// 丢弃(未完整 / 超预算等）。
    Discarded,
}

/// 脚本生命周期事件(script_lifecycle.event)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sqlx::Type)]
#[sqlx(rename_all = "snake_case")]
pub enum ScriptEvent {
    /// 首次加载。
    Load,

    /// 热重载成功。
    ReloadOk,

    /// 热重载失败。
    ReloadFail,

    /// 回调执行报错。
    CallbackError,

    /// 看门狗中止(超时 / 死循环）。
    WatchdogAbort,

    /// 配置告警。
    ConfigWarning,
}

/// 系统域事件:8 个变体一一对应系统域 8 张事件表。
///
/// 与行为域不同,系统域**不带 actor**;公共列 ts / session_id 在写入时由参数给定。
/// 带 [`SongId`] 的字段落库拆成 `ns` + `song_value`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    /// 取播放 URL(url_resolutions)。
    UrlResolution {
        /// 目标歌曲。
        song: SongId,

        /// 请求的音质档串。
        quality_requested: String,

        /// 结局。
        outcome: UrlOutcome,

        /// 是否为预取取链(非当前起播)。
        for_prefetch: bool,
    },

    /// 插件 hook 触发(hook_fires)。
    HookFire {
        /// 相关歌曲;不针对具体曲为 `None`。
        song: Option<SongId>,

        /// 触发的 hook。
        hook: HookKind,

        /// 触发时机。
        stage: HookStage,

        /// 裁决。
        decision: HookDecision,

        /// fail-open 原因;正常裁决为 `None`。
        fail_open: Option<FailOpen>,
    },

    /// 无缝边界裁决(gapless_boundaries)。
    GaplessBoundary {
        /// 衔接到的歌曲。
        song: SongId,

        /// 裁决结果。
        result: GaplessResult,
    },

    /// 预取(prefetches)。
    Prefetch {
        /// 预取的歌曲。
        song: SongId,

        /// 预取来源。
        source: PrefetchSource,

        /// 预取裁决。
        resolution: PrefetchResolution,
    },

    /// 边播边收割(cache_harvests)。
    CacheHarvest {
        /// 收割的歌曲。
        song: SongId,

        /// 音质档串。
        quality: String,

        /// 格式串。
        format: String,

        /// 落库结局。
        outcome: CacheHarvestOutcome,

        /// 收割字节数;未知为 `None`。
        bytes: Option<i64>,
    },

    /// 缓存淘汰(cache_evictions)。
    CacheEviction {
        /// 被淘汰的缓存 key。
        cache_key: String,

        /// 释放字节数。
        bytes: i64,
    },

    /// 脚本生命周期(script_lifecycle)。
    ScriptLifecycle {
        /// 事件类型。
        event: ScriptEvent,

        /// 细节文本;无为 `None`。
        detail: Option<String>,
    },

    /// 配置重载(config_reloads);无专有列。
    ConfigReload,
}

impl SystemEvent {
    /// 事件对应的目标表名(= [`crate::StatsEvent::kind_name`] 的系统域分支)。
    pub(crate) fn table(&self) -> &'static str {
        match self {
            Self::UrlResolution { .. } => "url_resolutions",
            Self::HookFire { .. } => "hook_fires",
            Self::GaplessBoundary { .. } => "gapless_boundaries",
            Self::Prefetch { .. } => "prefetches",
            Self::CacheHarvest { .. } => "cache_harvests",
            Self::CacheEviction { .. } => "cache_evictions",
            Self::ScriptLifecycle { .. } => "script_lifecycle",
            Self::ConfigReload => "config_reloads",
        }
    }

    /// 事件归属的来源 name(供 `exclude_sources` 在发送出口统一过滤)。
    ///
    /// 携带 [`SongId`] 的取其 namespace;缓存淘汰只有自由文本 key(不解析,防猜测格式
    /// 误伤)、脚本 / 配置生命周期为全局事件,均返回 `None`。
    ///
    /// # Return:
    ///   来源 name;事件不归属任何来源为 `None`
    pub(crate) fn source_name(&self) -> Option<&str> {
        match self {
            Self::UrlResolution { song, .. }
            | Self::GaplessBoundary { song, .. }
            | Self::Prefetch { song, .. }
            | Self::CacheHarvest { song, .. } => Some(song.namespace().name()),
            Self::HookFire { song, .. } => song.as_ref().map(|s| s.namespace().name()),
            Self::CacheEviction { .. } | Self::ScriptLifecycle { .. } | Self::ConfigReload => None,
        }
    }
}

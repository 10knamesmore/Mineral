//! Server-side script decisions between playback resolve and consuming open.

use std::time::Duration;

use mineral_model::{BitRate, MediaUrl, PlaybackMediaInfo, Song, SongId, StreamLayout};
use mineral_playback::{DirectMedia, DirectPreparedPlayback, PreparedPlayback};
use mineral_script::{
    BeforeDownloadCtx, BeforeStreamCtx, HookDecision, HookMode, RewriteSpec, ScriptSender,
};

use crate::player::PlayerCore;

/// Playability presented to the before-stream hook.
pub(crate) enum StreamAvailability<'a> {
    /// A prepared playback exists, with an optional direct capability.
    Playable(Option<&'a DirectMedia>),

    /// Provider lookup or resource resolution failed.
    Unplayable,
}

/// Script interception entry shared by playback and download orchestration.
pub(crate) struct HookGate {
    /// Script runtime sender; absence means unconditional continue.
    sender: Option<ScriptSender>,

    /// Immediate hook soft timeout.
    timeout: Duration,
}

impl HookGate {
    /// Creates a gate that always continues.
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            sender: None,
            timeout: Duration::ZERO,
        }
    }

    /// Creates a gate around a script runtime sender.
    ///
    /// # Params:
    ///   - `sender`: Script runtime sender.
    ///   - `timeout`: Immediate hook timeout.
    #[cfg(test)]
    pub(crate) fn with_sender(sender: ScriptSender, timeout: Duration) -> Self {
        Self {
            sender: Some(sender),
            timeout,
        }
    }

    /// Returns an attached sender when script interception is active.
    fn active(&self) -> Option<&ScriptSender> {
        self.sender.as_ref().filter(|sender| sender.is_attached())
    }

    /// Runs the download hook against an optional direct capability.
    ///
    /// # Params:
    ///   - `song`: Download target.
    ///   - `direct`: Optional direct capability of the prepared playback.
    pub(crate) async fn before_download(
        &self,
        song: &Song,
        direct: Option<&DirectMedia>,
    ) -> HookDecision {
        let Some(sender) = self.active() else {
            return HookDecision::Continue;
        };
        sender
            .intercept_download(
                BeforeDownloadCtx::playable(song.clone(), direct.cloned()),
                self.timeout,
            )
            .await
    }
}

impl PlayerCore {
    /// Creates the hook gate configured for this server.
    pub(crate) fn hook_gate(&self) -> HookGate {
        HookGate {
            sender: self.script_sender(),
            timeout: self.hook_timeout(),
        }
    }
}

/// Runs one before-stream decision without opening media.
///
/// # Params:
///   - `player`: Playback owner.
///   - `song`: Target song snapshot.
///   - `mode`: Immediate or gapless-prefetch commit point.
///   - `availability`: Explicit playable/unplayable state.
pub(crate) async fn decide_stream(
    player: &PlayerCore,
    song: &Song,
    mode: HookMode,
    availability: StreamAvailability<'_>,
) -> HookDecision {
    let gate = player.hook_gate();
    let Some(sender) = gate.active().cloned() else {
        return HookDecision::Continue;
    };
    let context = match availability {
        StreamAvailability::Playable(direct) => {
            BeforeStreamCtx::playable(song.clone(), mode, direct.cloned())
        }
        StreamAvailability::Unplayable => BeforeStreamCtx::unavailable(song.clone(), mode),
    };
    let timeout = match mode {
        HookMode::Immediate => gate.timeout,
        HookMode::Prefetch => prefetch_budget(player, gate.timeout),
    };
    let decision = sender.intercept_stream(context, timeout).await;
    record_hook_fire(player, &song.id, mode, &decision);
    decision
}

/// Builds a direct replacement plan from a script rewrite.
///
/// # Params:
///   - `song_id`: Domain identity retained by the replacement.
///   - `original`: Optional direct capability of the original prepared playback.
///   - `rewrite`: Script rewrite intent.
///
/// # Return:
///   A single-use direct replacement, or `None` when no direct location can be derived.
pub(crate) fn rewrite_prepared(
    song_id: &SongId,
    original: Option<&DirectMedia>,
    rewrite: &RewriteSpec,
) -> Option<Box<dyn PreparedPlayback>> {
    let original_info = original.map(DirectMedia::info);
    let url = rewrite
        .new_url()
        .cloned()
        .or_else(|| original.map(|value| value.locator().media_url()))?;
    let quality = rewrite
        .new_quality()
        .or_else(|| original_info.map(|value| value.quality))
        .unwrap_or(BitRate::Standard);
    let info = PlaybackMediaInfo {
        song_id: song_id.clone(),
        bitrate_bps: rewrite
            .bitrate_bps()
            .or_else(|| original_info.and_then(|value| value.bitrate_bps)),
        quality,
        size: original_info.and_then(|value| value.size),
        format: rewrite
            .format()
            .cloned()
            .or_else(|| original_info.and_then(|value| value.format.clone())),
        bit_depth: original_info.and_then(|value| value.bit_depth),
        substituted: rewrite.new_url().is_some()
            || original_info.is_some_and(|value| value.substituted),
    };
    let media = match url {
        MediaUrl::Remote(url) => {
            let headers = rewrite
                .stream_headers()
                .map(<[(String, String)]>::to_vec)
                .or_else(|| {
                    original.and_then(|value| {
                        value
                            .locator()
                            .remote()
                            .map(|remote| remote.headers().to_vec())
                    })
                })
                .unwrap_or_default();
            let layout = rewrite.layout().unwrap_or_else(|| {
                if rewrite.new_url().is_some() {
                    StreamLayout::Chunked
                } else {
                    original.map_or(StreamLayout::Chunked, DirectMedia::layout)
                }
            });
            DirectMedia::remote(info, url, headers, layout)
        }
        MediaUrl::Local(path) => DirectMedia::local(info, path),
    };
    Some(DirectPreparedPlayback::boxed(media))
}

/// Applies a hook Skip decision after the caller has revalidated instance ownership.
///
/// # Params:
///   - `player`: Playback owner.
///   - `song_id`: Skipped song identity.
///   - `mode`: Immediate or prefetch commit point.
///   - `reason`: Human-readable script reason.
pub(crate) fn apply_skip(player: &PlayerCore, song_id: &SongId, mode: HookMode, reason: &str) {
    match mode {
        HookMode::Immediate => {
            player.notify().toast(
                mineral_protocol::ToastKind::Warn,
                format!("脚本跳过播放:{reason}"),
            );
            player.next_song(mineral_stats::Actor::Script);
        }
        HookMode::Prefetch => veto_prefetch(player, song_id, reason),
    }
}

/// Emits the established playback-error completion for a failed current attempt.
///
/// # Params:
///   - `player`: Playback owner.
///   - `song`: Failed current song.
pub(crate) fn finish_failed(player: &PlayerCore, song: &Song) {
    player
        .notify()
        .track_finished(song, mineral_protocol::FinishReason::Error);
    player
        .inner
        .stats
        .play_ended(mineral_stats::FinishReason::Error, /*listen_ms*/ 0);
}

/// Returns a prefetch hook budget leaving time for open and audio arming.
fn prefetch_budget(player: &PlayerCore, immediate: Duration) -> Duration {
    Duration::from_millis(player.gapless_prefetch_ms().saturating_sub(2_000)).max(immediate)
}

/// Records one completed before-stream hook decision.
fn record_hook_fire(player: &PlayerCore, song: &SongId, mode: HookMode, decision: &HookDecision) {
    let stage = match mode {
        HookMode::Immediate => mineral_stats::HookStage::Immediate,
        HookMode::Prefetch => mineral_stats::HookStage::Prefetch,
    };
    let decision = match decision {
        HookDecision::Continue => mineral_stats::HookDecision::Continue,
        HookDecision::Rewrite(_) => mineral_stats::HookDecision::Rewrite,
        HookDecision::Skip { .. } => mineral_stats::HookDecision::Skip,
    };
    player.inner.stats.event(mineral_stats::StatsEvent::System(
        mineral_stats::SystemEvent::HookFire {
            song: Some(song.clone()),
            hook: mineral_stats::HookKind::BeforeStream,
            stage,
            decision,
            fail_open: None,
        },
    ));
}

/// Vetoes the still-pending next queue occurrence and cancels its slot.
fn veto_prefetch(player: &PlayerCore, song_id: &SongId, reason: &str) {
    player.with_state(|state| {
        if let Some(index) = crate::queue::next_index(state)
            && state
                .queue
                .get(index)
                .is_some_and(|song| song.id == *song_id)
        {
            state.prefetch_vetoed.push(index);
        }
        drop(state.prefetch.invalidate());
    });
    crate::gapless::record_prefetch(
        player,
        song_id.clone(),
        mineral_stats::PrefetchSource::Remote,
        mineral_stats::PrefetchResolution::Vetoed,
    );
    player.notify().toast(
        mineral_protocol::ToastKind::Warn,
        format!("脚本跳过下一首:{reason}"),
    );
}

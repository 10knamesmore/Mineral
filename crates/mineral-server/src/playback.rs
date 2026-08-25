//! Current and prefetch playback orchestration from local/provider resolve through audio handoff.

use mineral_model::Song;
use mineral_playback::{
    DirectMedia, DirectPreparedPlayback, OpenOptions, OpenedMedia, PlaybackRequest,
    PreparedPlayback,
};
use mineral_protocol::PlaybackOrigin;
use mineral_script::{HookDecision, HookMode};

use crate::download::Capturing;
use crate::hook_bridge::StreamAvailability;
use crate::playback_instance::PlaybackSlot;
use crate::player::PlayerCore;

/// Current or gapless-prefetch ownership slot.
#[derive(Clone, Copy)]
enum PlaybackRole {
    /// User-visible current playback attempt.
    Current,

    /// Background gapless-prefetch attempt.
    Prefetch,
}

impl PlaybackRole {
    /// Returns the corresponding hook commit point.
    fn hook_mode(self) -> HookMode {
        match self {
            Self::Current => HookMode::Immediate,
            Self::Prefetch => HookMode::Prefetch,
        }
    }

    /// Returns whether this role is prefetch for stats.
    fn is_prefetch(self) -> bool {
        match self {
            Self::Current => false,
            Self::Prefetch => true,
        }
    }
}

/// Resolved plan plus Mineral-owned playback policy facts.
struct ResolvedPlan {
    /// Single-use source or local prepared playback.
    prepared: Box<dyn PreparedPlayback>,

    /// Playback-origin fact retained for snapshots and stats.
    origin: PlaybackOrigin,

    /// Whether prepared bytes may enter the song-id/quality cache.
    cacheable: bool,

    /// Local path requiring envelope availability before playback.
    local_path: Option<std::path::PathBuf>,
}

/// Provider/local resolution state presented to the hook exactly once.
enum Resolution {
    /// A single-use prepared playback is available.
    Playable(ResolvedPlan),

    /// Resolution failed before a prepared playback existed.
    Unplayable,
}

/// Starts resolve/hook/open for a current playback slot.
///
/// # Params:
///   - `player`: Playback owner.
///   - `song`: Current song snapshot.
///   - `slot`: Active current ownership slot.
///   - `local_hit`: Mineral-owned local resolution result computed at intent time.
pub(crate) fn start_current(
    player: &PlayerCore,
    song: Song,
    slot: PlaybackSlot,
    local_hit: Option<crate::resolve::LocalMediaHit>,
) {
    spawn(player, song, slot, PlaybackRole::Current, local_hit);
}

/// Starts resolve/hook/open for a gapless-prefetch slot.
///
/// # Params:
///   - `player`: Playback owner.
///   - `song`: Prefetched song snapshot.
///   - `slot`: Active prefetch ownership slot.
pub(crate) fn start_prefetch(player: &PlayerCore, song: Song, slot: PlaybackSlot) {
    spawn(player, song, slot, PlaybackRole::Prefetch, None);
}

/// Spawns one cancellation-scoped playback orchestration task.
fn spawn(
    player: &PlayerCore,
    song: Song,
    slot: PlaybackSlot,
    role: PlaybackRole,
    local_hit: Option<crate::resolve::LocalMediaHit>,
) {
    let player = player.clone();
    tokio::spawn(async move {
        run(&player, song, slot, role, local_hit).await;
    });
}

/// Executes resolve, hook, open, guard, and audio handoff for one slot.
async fn run(
    player: &PlayerCore,
    song: Song,
    slot: PlaybackSlot,
    role: PlaybackRole,
    local_hit: Option<crate::resolve::LocalMediaHit>,
) {
    let resolution = match resolve(player, &song, &slot, local_hit).await {
        Ok(plan) => {
            player.record_stream_resolution(
                &song.id,
                mineral_stats::StreamOutcome::Ok,
                role.is_prefetch(),
            );
            Resolution::Playable(plan)
        }
        Err(error) => {
            if slot.cancellation.is_cancelled() || !matches_slot(player, &slot, role) {
                return;
            }
            mineral_log::warn!(
                target: "playback",
                song_id = %song.id.qualified(),
                error = mineral_log::chain(&error),
                "playback resolve failed"
            );
            player.record_stream_resolution(
                &song.id,
                mineral_stats::StreamOutcome::Error,
                role.is_prefetch(),
            );
            Resolution::Unplayable
        }
    };
    if !matches_slot(player, &slot, role) {
        return;
    }
    let availability = match &resolution {
        Resolution::Playable(plan) => StreamAvailability::Playable(plan.prepared.direct_media()),
        Resolution::Unplayable => StreamAvailability::Unplayable,
    };
    let decision =
        crate::hook_bridge::decide_stream(player, &song, role.hook_mode(), availability).await;
    if !matches_slot(player, &slot, role) {
        return;
    }
    let Some(plan) = select_plan(player, &song, role, resolution, decision) else {
        return;
    };
    let direct = plan.prepared.direct_media().cloned();
    let ResolvedPlan {
        prepared,
        origin,
        cacheable,
        local_path,
    } = plan;
    let options = OpenOptions::new(
        slot.cancellation.child_token(),
        player.playback_prefetch_bytes(),
    );
    let opened = tokio::select! {
        biased;
        () = slot.cancellation.cancelled() => return,
        result = prepared.open(options) => match result {
            Ok(opened) => opened,
            Err(error) => {
                if !slot.cancellation.is_cancelled() && matches_slot(player, &slot, role) {
                    mineral_log::warn!(
                        target: "playback",
                        song_id = %song.id.qualified(),
                        error = mineral_log::chain(&error),
                        "prepared playback open failed"
                    );
                    finish_unplayable(player, &song, role);
                }
                return;
            }
        },
    };
    if !matches_slot(player, &slot, role) {
        drop(opened);
        return;
    }
    if let Some(path) = &local_path {
        player.ensure_envelope(song.id.clone(), path.clone());
    }
    match role {
        PlaybackRole::Current => {
            start_opened_current(player, &song, &slot, opened, direct, origin, cacheable)
        }
        PlaybackRole::Prefetch => {
            crate::gapless::arm_opened(player, song, &slot, opened, direct, origin, cacheable)
        }
    }
}

/// Applies one hook decision to playable or unplayable resolution.
///
/// # Params:
///   - `player`: Playback owner.
///   - `song`: Target song snapshot.
///   - `role`: Current or prefetch behavior on Skip/failure.
///   - `resolution`: Provider/local resolution outcome.
///   - `decision`: Single hook decision for the attempt.
fn select_plan(
    player: &PlayerCore,
    song: &Song,
    role: PlaybackRole,
    resolution: Resolution,
    decision: HookDecision,
) -> Option<ResolvedPlan> {
    match decision {
        HookDecision::Continue => match resolution {
            Resolution::Playable(plan) => Some(plan),
            Resolution::Unplayable => {
                finish_unplayable(player, song, role);
                None
            }
        },
        HookDecision::Rewrite(rewrite) => {
            let original = match &resolution {
                Resolution::Playable(plan) => plan.prepared.direct_media(),
                Resolution::Unplayable => None,
            };
            let Some(prepared) = crate::hook_bridge::rewrite_prepared(&song.id, original, &rewrite)
            else {
                finish_unplayable(player, song, role);
                return None;
            };
            Some(ResolvedPlan {
                prepared,
                origin: PlaybackOrigin::Remote,
                cacheable: false,
                local_path: None,
            })
        }
        HookDecision::Skip { reason } => {
            crate::hook_bridge::apply_skip(player, &song.id, role.hook_mode(), &reason);
            None
        }
    }
}

/// Resolves local media first, then the song source provider.
async fn resolve(
    player: &PlayerCore,
    song: &Song,
    slot: &PlaybackSlot,
    local_hit: Option<crate::resolve::LocalMediaHit>,
) -> color_eyre::Result<ResolvedPlan> {
    let local_hit = local_hit.or_else(|| {
        crate::resolve::resolve_local(
            player.media_cache(),
            player.music_dir(),
            song,
            player.playback_quality(),
        )
    });
    if let Some(hit) = local_hit {
        let media = crate::resolve::local_media(song, &hit.path, hit.quality);
        return Ok(ResolvedPlan {
            prepared: DirectPreparedPlayback::boxed(media),
            origin: hit.origin,
            cacheable: false,
            local_path: Some(hit.path),
        });
    }
    let provider = player
        .playback()
        .get(song.source())
        .ok_or_else(|| color_eyre::eyre::eyre!("no playback provider for {:?}", song.source()))?;
    let request = PlaybackRequest::new(song.id.clone(), player.playback_quality());
    let prepared = tokio::select! {
        biased;
        () = slot.cancellation.cancelled() => {
            return Err(color_eyre::eyre::eyre!("playback resolve cancelled"));
        }
        result = provider.resolve(request, slot.cancellation.child_token()) => result?,
    };
    Ok(ResolvedPlan {
        prepared,
        origin: PlaybackOrigin::Remote,
        cacheable: true,
        local_path: None,
    })
}

/// Starts already-opened current media and updates snapshot/stats facts.
fn start_opened_current(
    player: &PlayerCore,
    song: &Song,
    slot: &PlaybackSlot,
    opened: OpenedMedia,
    direct: Option<DirectMedia>,
    origin: PlaybackOrigin,
    cacheable: bool,
) {
    let info = opened.info().clone();
    let capture = cacheable
        .then(|| {
            player
                .media_cache()
                .capture_path(&song.id, player.playback_quality())
        })
        .flatten();
    let started = player.with_state(|state| {
        if !state
            .current_slot
            .as_ref()
            .is_some_and(|active| active.matches(slot.instance_id, &slot.song_id))
        {
            return false;
        }
        match capture {
            Some(path) => {
                player.audio().play_capturing(opened, path.clone());
                state.capturing = Some(Capturing {
                    song: song.clone(),
                    quality: player.playback_quality(),
                    format: info.format.clone(),
                    path,
                });
            }
            None => player.audio().play(opened),
        }
        state.play_origin = Some(origin);
        state.media_info = Some(info.clone());
        state.direct_media = direct;
        state.bump_current();
        true
    });
    if started {
        player.enrich_from_media_info(&info);
    }
}

/// Returns whether the active role slot still owns an async completion.
fn matches_slot(player: &PlayerCore, slot: &PlaybackSlot, role: PlaybackRole) -> bool {
    player.with_state(|state| {
        let active = match role {
            PlaybackRole::Current => state.current_slot.as_ref(),
            PlaybackRole::Prefetch => state.prefetch.slot(),
        };
        active.is_some_and(|active| active.matches(slot.instance_id, &slot.song_id))
    })
}

/// Applies the established unresolved/open-failure behavior for one role.
fn finish_unplayable(player: &PlayerCore, song: &Song, role: PlaybackRole) {
    match role {
        PlaybackRole::Current => crate::hook_bridge::finish_failed(player, song),
        PlaybackRole::Prefetch => {}
    }
}

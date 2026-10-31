use mineral_channel_core::PlaylistLoad;
use mineral_model::{CachePolicy, RefreshScope};
use parking_lot::Mutex;
use rustc_hash::FxHashSet;

use crate::{ChannelFetchKind, TaskEvent};

/// A category invalidation stays pending for each detail until a complete fresh read succeeds.
#[derive(Default)]
pub(crate) struct ReadCache {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    invalidated: Vec<RefreshScope>,

    reusable: FxHashSet<ChannelFetchKind>,
}

impl ReadCache {
    pub(crate) fn policy(&self, kind: &ChannelFetchKind) -> CachePolicy {
        let state = self.state.lock();
        if state
            .invalidated
            .iter()
            .any(|scope| kind.affected_by(scope))
            && !state.reusable.contains(&cache_key(kind))
        {
            CachePolicy::Reload
        } else {
            CachePolicy::UseCache
        }
    }

    pub(crate) fn invalidate(&self, scope: &RefreshScope) {
        let mut state = self.state.lock();
        state.reusable.retain(|kind| !kind.affected_by(scope));
        if !state.invalidated.contains(scope) {
            state.invalidated.push(scope.clone());
        }
    }

    /// Called for each successful Reload result under the publication lock, after checking cancellation.
    pub(crate) fn record_fresh_result(&self, kind: &ChannelFetchKind, event: &TaskEvent) {
        if matches!(kind, ChannelFetchKind::PlaylistDetail { .. })
            && !matches!(event, TaskEvent::PlaylistDetailFetched { detail, fresh: true, .. } if detail.complete)
        {
            return;
        }
        if matches!(event, TaskEvent::AlbumDetailFetched { fresh: false, .. }) {
            return;
        }
        self.state.lock().reusable.insert(cache_key(kind));
    }
}

fn cache_key(kind: &ChannelFetchKind) -> ChannelFetchKind {
    match kind {
        ChannelFetchKind::PlaylistDetail { id, .. } => ChannelFetchKind::PlaylistDetail {
            id: id.clone(),
            load: PlaylistLoad::Complete,
        },
        _ => kind.clone(),
    }
}

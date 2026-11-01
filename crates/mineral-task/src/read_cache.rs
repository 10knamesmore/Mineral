use mineral_common::revision::Revision;
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
    /// Latest invalidation for each scope, ordered by revision.
    invalidated: Vec<(RefreshScope, Revision)>,

    /// Increases on every refresh, including repeated refreshes of the same scope.
    revision: Revision,

    reusable: FxHashSet<ChannelFetchKind>,
}

impl ReadCache {
    pub(crate) fn policy(&self, kind: &ChannelFetchKind) -> CachePolicy {
        self.read_state(kind).0
    }

    /// Captures policy and invalidation revision together for a stream that may pause.
    pub(crate) fn read_state(&self, kind: &ChannelFetchKind) -> (CachePolicy, Option<Revision>) {
        let state = self.state.lock();
        let revision = state.revision_for(kind);
        let policy = if revision.is_some() && !state.reusable.contains(kind) {
            CachePolicy::Reload
        } else {
            CachePolicy::UseCache
        };
        (policy, revision)
    }

    pub(crate) fn invalidate(&self, scope: &RefreshScope) {
        let mut state = self.state.lock();
        state.reusable.retain(|kind| !kind.affected_by(scope));
        state.revision = state.revision.next();
        let revision = state.revision;
        state.invalidated.retain(|(old, _)| old != scope);
        state.invalidated.push((scope.clone(), revision));
    }

    /// A paused stream cannot publish or make its old cache reusable after refresh.
    pub(crate) fn accept_result(
        &self,
        kind: &ChannelFetchKind,
        revision: Option<Revision>,
        complete: bool,
    ) -> bool {
        let mut state = self.state.lock();
        if state.revision_for(kind) != revision {
            return false;
        }
        if complete {
            state.reusable.insert(kind.clone());
        }
        true
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
        self.state.lock().reusable.insert(kind.clone());
    }
}

impl State {
    fn revision_for(&self, kind: &ChannelFetchKind) -> Option<Revision> {
        self.invalidated
            .iter()
            .rev()
            .find(|(scope, _)| kind.affected_by(scope))
            .map(|(_, revision)| *revision)
    }
}

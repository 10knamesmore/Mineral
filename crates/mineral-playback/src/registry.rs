//! Immutable provider registry keyed by open source identity.

use std::sync::Arc;

use mineral_model::SourceKind;
use rustc_hash::FxHashMap;

use crate::{Error, PlaybackProvider, Result};

/// Immutable process-local playback provider registry.
#[derive(Clone, Default)]
pub struct PlaybackRegistry {
    /// Providers keyed by the source they serve.
    providers: Arc<FxHashMap<SourceKind, Arc<dyn PlaybackProvider>>>,
}

impl PlaybackRegistry {
    /// Builds a registry and rejects duplicate source registrations.
    ///
    /// # Params:
    ///   - `providers`: Provider handles to register.
    ///
    /// # Return:
    ///   An immutable registry, or an error when two providers serve the same source.
    pub fn new(providers: Vec<Arc<dyn PlaybackProvider>>) -> Result<Self> {
        let mut by_source = FxHashMap::<SourceKind, Arc<dyn PlaybackProvider>>::default();
        for provider in providers {
            let source = provider.source();
            if by_source.insert(source, provider).is_some() {
                return Err(Error::DuplicateProvider {
                    source_kind: source,
                });
            }
        }
        Ok(Self {
            providers: Arc::new(by_source),
        })
    }

    /// Returns an empty registry.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Returns the provider serving a source.
    ///
    /// # Params:
    ///   - `source`: Source identity to look up.
    #[must_use]
    pub fn get(&self, source: SourceKind) -> Option<Arc<dyn PlaybackProvider>> {
        self.providers.get(&source).cloned()
    }
}

//! Holds the account connection, album cache policy and lyric preferences for metadata reads.

use std::sync::Arc;

use mineral_channel_core::store::NamespaceStore;

use super::albums::AlbumLoader;
use crate::AppleMusicConfig;
use crate::account::AccountClient;

/// Metadata operations for one channel, sharing authentication with playback.
/// Album caching and lyric preferences belong here rather than to the HTTP client.
pub(crate) struct MetadataClient {
    pub(super) account: Arc<AccountClient>,

    pub(super) albums: AlbumLoader,

    pub(super) lyrics_translation_language: Option<String>,

    pub(super) lyrics_romanization_script: Option<String>,
}

impl MetadataClient {
    pub(crate) fn new(
        account: Arc<AccountClient>,
        config: &AppleMusicConfig,
        persist: Option<Arc<dyn NamespaceStore>>,
    ) -> Self {
        Self {
            account,
            albums: AlbumLoader::new(config.album_cache(), persist),
            lyrics_translation_language: config.lyrics_translation_language().clone(),
            lyrics_romanization_script: config.lyrics_romanization_script().clone(),
        }
    }
}

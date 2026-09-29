//! Rust-scanned local files, directory playlists and direct playback.

mod catalog;
mod channel;
mod error;
mod metadata;
mod scan;

use std::{path::PathBuf, sync::Arc};

use arc_swap::ArcSwapOption;
use mineral_model::SourceKind;
use mineral_persist::ServerStore;
use tokio::sync::Mutex;

use catalog::Catalog;
use error::Result;

/// Catalog adapter and playback provider sharing one catalog for this channel instance.
pub struct LocalLibrary {
    /// Readers keep one complete snapshot while a newer one can be published.
    catalog: ArcSwapOption<Catalog>,

    /// Concurrent first reads perform one scan.
    load_lock: Mutex<()>,

    /// Shared song metadata storage.
    persist: ServerStore,

    /// Directory for artwork extracted from audio tags.
    cover_dir: PathBuf,

    /// Root paths captured at daemon startup.
    roots: Vec<PathBuf>,
}

impl LocalLibrary {
    /// Capture startup configuration without scanning directories.
    /// The first successful channel request loads the catalog.
    ///
    /// # Params:
    ///   - `persist`: Generic song metadata storage.
    ///   - `cover_dir`: Extracted artwork directory, separate from audio downloads/cache.
    ///   - `roots`: Startup root paths, including the `~/` shorthand.
    ///
    pub fn new(persist: ServerStore, cover_dir: PathBuf, roots: Vec<PathBuf>) -> Self {
        Self {
            catalog: ArcSwapOption::empty(),
            load_lock: Mutex::new(()),
            persist,
            cover_dir,
            roots,
        }
    }

    /// Return a stable snapshot, loading it on the first successful read.
    async fn catalog(&self) -> Result<Arc<Catalog>> {
        if let Some(catalog) = self.catalog.load_full() {
            return Ok(catalog);
        }
        let _loading = self.load_lock.lock().await;
        if let Some(catalog) = self.catalog.load_full() {
            return Ok(catalog);
        }
        mineral_log::info!(target: "local_library", roots = self.roots.len(), "loading local catalog");
        let catalog = match self.load_catalog().await {
            Ok(catalog) => {
                mineral_log::info!(target: "local_library", playlists = catalog.playlists.len(), songs = catalog.files.len(), "local catalog loaded");
                Arc::new(catalog)
            }
            Err(error) => {
                mineral_log::warn!(target: "local_library", error = mineral_log::chain(&error), "local catalog load failed");
                return Err(error);
            }
        };
        self.catalog.store(Some(Arc::clone(&catalog)));
        Ok(catalog)
    }

    /// Scan on a blocking worker, then update generic song metadata.
    async fn load_catalog(&self) -> Result<Catalog> {
        let roots = self.roots.clone();
        let cover_dir = self.cover_dir.clone();
        let catalog = tokio::task::spawn_blocking(move || {
            let context = scan::run(&roots, cover_dir)?;
            scan::build(context)
        })
        .await??;
        let songs = catalog
            .files
            .values()
            .map(|file| &file.song)
            .collect::<Vec<_>>();
        self.persist
            .scope(SourceKind::LOCAL)
            .replace_meta_batch(&songs)
            .await?;
        Ok(catalog)
    }
}

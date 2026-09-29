//! Offline maintenance of cover files indexed by the TUI database.

use crate::persistence::{Result, TuiStore};
use mineral_persist::CacheStats;
use std::path::{Path, PathBuf};

/// Reads cover-cache contents; the database parent directory must exist.
pub async fn snapshot(db_path: &Path, root: PathBuf, capacity: u64) -> Result<CacheStats> {
    Ok(TuiStore::open(db_path)
        .await?
        .cover_cache(root, capacity)
        .await?
        .snapshot())
}

/// Removes cover files and index entries, preserving preferences and cursor positions.
pub async fn clear(db_path: &Path, root: PathBuf) -> Result<CacheStats> {
    let index = TuiStore::open(db_path).await?.cover_cache(root, 0).await?;
    Ok(index.clear().await?)
}

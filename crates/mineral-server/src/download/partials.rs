//! Attempt-owned partial paths and cleanup after transfer or daemon restart.

use std::path::{Path, PathBuf};

use mineral_protocol::DownloadId;

/// A crash-leftover partial could not be inspected or removed.
#[derive(Debug, thiserror::Error)]
enum CleanupError {
    /// Filesystem operation failed at the indicated path.
    #[error("partial cleanup failed at {path:?}")]
    File {
        /// Path being inspected or removed.
        path: PathBuf,

        /// Underlying filesystem error.
        #[source]
        source: std::io::Error,
    },
}

/// Builds the partial path owned by this download's sole execution.
pub(super) fn owned_partial_path(export: &Path, id: &DownloadId) -> PathBuf {
    let extension = export
        .extension()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("media");
    export.with_extension(format!("{extension}.mineral-{}.part-dl", id.as_str()))
}

/// Removes one known owned partial, ignoring an already-absent file.
pub(super) async fn remove_owned_partial(path: &Path) {
    match tokio::fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            mineral_log::warn!(target: "download", path = %path.display(), error = mineral_log::chain(&error), "failed to remove owned partial");
        }
    }
}

/// Removes crash leftovers before the manager accepts any new work.
pub(crate) fn cleanup_orphan_partials(root: &Path) {
    match cleanup_owned_tree(root) {
        Ok(removed) if removed > 0 => {
            mineral_log::info!(target: "download", root = %root.display(), removed, "orphan download partials removed");
        }
        Ok(_) => {}
        Err(error) => {
            mineral_log::warn!(target: "download", root = %root.display(), error = mineral_log::chain(&error), "orphan partial cleanup failed");
        }
    }
}

/// Recursively removes only files matching Mineral's owned partial suffix contract.
fn cleanup_owned_tree(root: &Path) -> Result<usize, CleanupError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => {
            return Err(CleanupError::File {
                path: root.to_path_buf(),
                source: error,
            });
        }
    };
    let mut removed = 0usize;
    for entry in entries {
        let entry = entry.map_err(|source| CleanupError::File {
            path: root.to_path_buf(),
            source,
        })?;
        let file_type = entry.file_type().map_err(|source| CleanupError::File {
            path: entry.path(),
            source,
        })?;
        let path = entry.path();
        if file_type.is_dir() {
            removed = removed.saturating_add(cleanup_owned_tree(&path)?);
        } else if file_type.is_file() && is_owned_partial(&path) {
            std::fs::remove_file(&path).map_err(|source| CleanupError::File {
                path: path.clone(),
                source,
            })?;
            removed = removed.saturating_add(1);
        }
    }
    Ok(removed)
}

/// Whether a file name proves it belongs to the manager's unique partial contract.
fn is_owned_partial(path: &Path) -> bool {
    path.file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(|name| name.contains(".mineral-") && name.ends_with(".part-dl"))
}

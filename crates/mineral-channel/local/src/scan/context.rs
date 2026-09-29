//! Filesystem facts collected by the blocking scanner.

use std::path::{Path, PathBuf};

use rustc_hash::FxHashMap;

use crate::error::{Error, Result};
use crate::metadata::{self, Metadata};

/// One scan's immutable input after traversal and metadata probing finish.
pub(crate) struct ScanContext {
    /// Parsed facts for every recognized file in this scan.
    pub(super) probes: FxHashMap<PathBuf, Metadata>,

    /// Extracted artwork directory.
    pub(super) cover_dir: PathBuf,
}

impl ScanContext {
    /// Probe the current file entirely on the Rust worker.
    pub fn probe(&mut self, path: &Path) -> Result<()> {
        let metadata = metadata::probe(path, &self.cover_dir)?;
        self.probes.insert(path.to_owned(), metadata);
        Ok(())
    }
}

/// Expand and canonicalize roots, keeping only the shallowest configured directories.
pub(super) fn roots(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut roots = Vec::<PathBuf>::new();
    for path in paths {
        let path = if let Ok(relative) = path.strip_prefix("~") {
            PathBuf::from(
                std::env::var_os("HOME")
                    .ok_or_else(|| Error::HomeUnavailable { path: path.clone() })?,
            )
            .join(relative)
        } else {
            path.clone()
        };
        if !path.is_absolute() {
            return Err(Error::RelativeRoot { path });
        }
        let path = path.canonicalize().map_err(|source| Error::File {
            operation: "canonicalize local root",
            path: path.clone(),
            source,
        })?;
        if !path.is_dir() {
            return Err(Error::RootNotDirectory { path });
        }
        if roots.iter().any(|root| path.starts_with(root)) {
            continue;
        }
        roots.retain(|root| !root.starts_with(&path));
        roots.push(path);
    }
    Ok(roots)
}

//! Recursive discovery and tag probing run entirely in a Rust blocking worker.

use std::path::PathBuf;

use walkdir::WalkDir;

use super::context::{self, ScanContext};
use crate::metadata;

/// Collect all recognized files without following symlinks.
pub(crate) fn run(roots: &[PathBuf], cover_dir: PathBuf) -> color_eyre::Result<ScanContext> {
    let roots = context::roots(roots)?;
    let mut context = ScanContext {
        probes: Default::default(),
        cover_dir,
    };
    for root in roots {
        for entry in WalkDir::new(root).follow_links(false).sort_by_file_name() {
            let entry = entry?;
            if entry.file_type().is_file() && metadata::supported(entry.path()) {
                context.probe(entry.path())?;
            }
        }
    }
    mineral_log::debug!(target: "local_library", files = context.probes.len(), "filesystem scan completed");
    Ok(context)
}

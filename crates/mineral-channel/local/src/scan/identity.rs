//! Stable local IDs derived without retaining a previous scan.

use std::path::Path;

use mineral_model::{AlbumId, PlaylistId, SongId, SourceKind};
use sha2::{Digest, Sha256};

/// The canonical absolute path alone defines a local song's identity.
pub(super) fn song(path: &Path) -> SongId {
    SongId::new(SourceKind::LOCAL, path_digest(path))
}

/// The canonical directory path defines a local playlist's identity.
pub(super) fn playlist(path: &Path) -> PlaylistId {
    PlaylistId::new(SourceKind::LOCAL, path_digest(path))
}

/// Hash the platform's lossless path representation, not its display text.
fn path_digest(path: &Path) -> String {
    let mut digest = Sha256::new();
    update_path(&mut digest, path);
    hex::encode(digest.finalize())
}

#[cfg(unix)]
/// Unix paths retain their exact filesystem byte sequence.
fn update_path(digest: &mut Sha256, path: &Path) {
    use std::os::unix::ffi::OsStrExt;

    digest.update(path.as_os_str().as_bytes());
}

#[cfg(windows)]
/// Windows paths retain every UTF-16 code unit, including unpaired surrogates.
fn update_path(digest: &mut Sha256, path: &Path) {
    use std::os::windows::ffi::OsStrExt;

    for unit in path.as_os_str().encode_wide() {
        digest.update(unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, windows)))]
compile_error!("local path identity requires a Unix or Windows target");

/// The tagged album name and ordered album artists define a local album.
pub(super) fn album(name: &str, artists: &[String]) -> Result<AlbumId, std::num::TryFromIntError> {
    let mut digest = Sha256::new();
    update_field(&mut digest, name)?;
    for artist in artists {
        update_field(&mut digest, artist)?;
    }
    Ok(AlbumId::new(
        SourceKind::LOCAL,
        hex::encode(digest.finalize()),
    ))
}

/// Feed one length-delimited name into the album identity.
fn update_field(digest: &mut Sha256, value: &str) -> Result<(), std::num::TryFromIntError> {
    digest.update(u64::try_from(value.len())?.to_be_bytes());
    digest.update(value.as_bytes());
    Ok(())
}

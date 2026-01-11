use std::path::PathBuf;

pub(super) fn home_dir() -> PathBuf {
    home::home_dir().expect("could not determine home directory")
}

/// ~/.cache
pub(super) fn cache_dir() -> PathBuf {
    home_dir().join(".cache")
}

/// ~/.config
pub(super) fn config_dir() -> PathBuf {
    home_dir().join(".config")
}

/// ~/.local/share
pub(super) fn data_dir() -> PathBuf {
    home_dir().join(".local").join("share")
}

/// ~/.local/state
pub(super) fn state_dir() -> PathBuf {
    home_dir().join(".local").join("state")
}

/// ~/Music
pub(super) fn audio_dir() -> PathBuf {
    home_dir().join("Music")
}

/// ~/Downloads
pub(super) fn download_dir() -> PathBuf {
    home_dir().join("Downloads")
}

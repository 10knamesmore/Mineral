use std::{env, path::PathBuf};

fn env_dir(var: &str) -> PathBuf {
    env::var_os(var)
        .map(PathBuf::from)
        .unwrap_or_else(|| panic!("{var} is not set"))
}

/// %USERPROFILE%
pub(super) fn home_dir() -> PathBuf {
    env_dir("USERPROFILE")
}

/// %LOCALAPPDATA%
pub(super) fn cache_dir() -> PathBuf {
    env_dir("LOCALAPPDATA")
}

/// %APPDATA%
pub(super) fn config_dir() -> PathBuf {
    env_dir("APPDATA")
}

/// %LOCALAPPDATA%
pub(super) fn data_dir() -> PathBuf {
    env_dir("LOCALAPPDATA")
}

/// %LOCALAPPDATA%
pub(super) fn state_dir() -> PathBuf {
    env_dir("LOCALAPPDATA")
}

/// %USERPROFILE%\Music
pub(super) fn audio_dir() -> PathBuf {
    home_dir().join("Music")
}

/// %USERPROFILE%\Downloads
pub(super) fn download_dir() -> PathBuf {
    home_dir().join("Downloads")
}

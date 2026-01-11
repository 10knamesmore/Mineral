use std::{env, path::PathBuf};

pub fn home_dir() -> PathBuf {
    home::home_dir().expect("could not determine home directory")
}

fn xdg_dir(var: &str, fallback: PathBuf) -> PathBuf {
    env::var_os(var).map(PathBuf::from).unwrap_or(fallback)
}

/// ~/.cache (or $XDG_CACHE_HOME)
pub fn cache_dir() -> PathBuf {
    xdg_dir("XDG_CACHE_HOME", home_dir().join(".cache"))
}

/// ~/.config (or $XDG_CONFIG_HOME)
pub fn config_dir() -> PathBuf {
    xdg_dir("XDG_CONFIG_HOME", home_dir().join(".config"))
}

/// ~/.local/share (or $XDG_DATA_HOME)
pub fn data_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", home_dir().join(".local").join("share"))
}

/// ~/.local/state (or $XDG_STATE_HOME)
pub fn state_dir() -> PathBuf {
    xdg_dir("XDG_STATE_HOME", home_dir().join(".local").join("state"))
}

/// ~/Music
pub fn audio_dir() -> PathBuf {
    home_dir().join("Music")
}

/// ~/Downloads（同上）
pub fn download_dir() -> PathBuf {
    home_dir().join("Downloads")
}

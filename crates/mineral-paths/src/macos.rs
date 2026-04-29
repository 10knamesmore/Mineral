//! macOS 平台路径实现。

use std::path::PathBuf;

use color_eyre::eyre::eyre;

fn home_dir() -> color_eyre::Result<PathBuf> {
    let h = std::env::var_os("HOME").ok_or_else(|| eyre!("HOME 未设置，无法确定 mineral 目录"))?;
    Ok(PathBuf::from(h))
}

fn xdg(env: &str, fallback: &str) -> color_eyre::Result<PathBuf> {
    if let Some(v) = std::env::var_os(env).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(v).join("mineral"));
    }
    Ok(home_dir()?.join(fallback).join("mineral"))
}

pub(crate) fn config_dir() -> color_eyre::Result<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub(crate) fn data_dir() -> color_eyre::Result<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub(crate) fn cache_dir() -> color_eyre::Result<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache")
}

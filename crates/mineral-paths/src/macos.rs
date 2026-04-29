//! macOS 平台路径实现。
//!
//! 当前与 [`crate::linux`] 共用 XDG 布局——刻意不沿用
//! `~/Library/Application Support` / `~/Library/Caches` 的 Apple 惯例。后续若想分叉，
//! 改本文件即可，不必动 `lib.rs` 的公共 API。

use std::path::PathBuf;

use anyhow::{anyhow, Result};

fn home_dir() -> Result<PathBuf> {
    let h = std::env::var_os("HOME")
        .ok_or_else(|| anyhow!("HOME 未设置，无法确定 mineral 目录"))?;
    Ok(PathBuf::from(h))
}

fn xdg(env: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(v) = std::env::var_os(env).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(v).join("mineral"));
    }
    Ok(home_dir()?.join(fallback).join("mineral"))
}

pub(crate) fn config_dir() -> Result<PathBuf> {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub(crate) fn data_dir() -> Result<PathBuf> {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub(crate) fn cache_dir() -> Result<PathBuf> {
    xdg("XDG_CACHE_HOME", ".cache")
}

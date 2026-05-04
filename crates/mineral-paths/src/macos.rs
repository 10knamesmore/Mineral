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

/// macOS runtime 目录:`$TMPDIR/mineral`,或 fallback `/tmp/mineral`。
///
/// 用于 IPC unix socket 等「进程级生命周期」的 ephemeral 文件。
/// macOS 没有 `XDG_RUNTIME_DIR`,但 shell 环境总会有 `TMPDIR`(通常
/// `/var/folders/.../T/`)。调用方负责 `create_dir_all` 与权限收紧。
pub(crate) fn runtime_dir() -> color_eyre::Result<PathBuf> {
    if let Some(v) = std::env::var_os("TMPDIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(v).join("mineral"));
    }
    Ok(PathBuf::from("/tmp/mineral"))
}

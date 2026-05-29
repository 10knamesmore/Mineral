//! XDG Base Directory 解析。**所有 unix(含 macOS)统一走 XDG**:开发者向 / terminal-first
//! 工具的惯例(helix / yazi / alacritty 等皆如此),与 vim/git 一致、被 dotfile 管理器纳管,
//! 而非 macOS 的 `~/Library/...`(那是 GUI app 的约定)。

use std::path::{Path, PathBuf};

use color_eyre::eyre::eyre;

/// 解析 `$HOME`;未设(罕见)返回 `Err`。
///
/// # Return:
///   `$HOME` 路径。
pub(crate) fn home_dir() -> color_eyre::Result<PathBuf> {
    let h = std::env::var_os("HOME").ok_or_else(|| eyre!("HOME 未设置，无法确定 mineral 目录"))?;
    Ok(PathBuf::from(h))
}

/// 通用 XDG 解析:`$<env>/mineral`(仅当其为**绝对路径**,符合 XDG 规范——相对值当未设),
/// 否则 `$HOME/<fallback>/mineral`。
///
/// # Params:
///   - `env`: XDG 环境变量名(如 `XDG_CONFIG_HOME`)
///   - `fallback`: 缺失时相对 `$HOME` 的子目录(如 `.config`)
///
/// # Return:
///   解析得到的目录路径。
fn xdg_base(env: &str, fallback: &str) -> color_eyre::Result<PathBuf> {
    if let Some(v) = std::env::var_os(env).filter(|v| !v.is_empty())
        && Path::new(&v).is_absolute()
    {
        return Ok(PathBuf::from(v).join("mineral"));
    }
    Ok(home_dir()?.join(fallback).join("mineral"))
}

/// 配置根目录(`$XDG_CONFIG_HOME/mineral` 或 `~/.config/mineral`)。
///
/// # Return:
///   解析得到的目录路径。
pub(crate) fn config_dir() -> color_eyre::Result<PathBuf> {
    xdg_base("XDG_CONFIG_HOME", ".config")
}

/// 数据根目录(`$XDG_DATA_HOME/mineral` 或 `~/.local/share/mineral`)。
///
/// # Return:
///   解析得到的目录路径。
pub(crate) fn data_dir() -> color_eyre::Result<PathBuf> {
    xdg_base("XDG_DATA_HOME", ".local/share")
}

/// 缓存根目录(`$XDG_CACHE_HOME/mineral` 或 `~/.cache/mineral`)。
///
/// # Return:
///   解析得到的目录路径。
pub(crate) fn cache_dir() -> color_eyre::Result<PathBuf> {
    xdg_base("XDG_CACHE_HOME", ".cache")
}

/// 用户音乐目录:`$XDG_MUSIC_DIR`(绝对)或 `~/Music`(**不含** mineral 子目录)。
/// macOS 通常无 `XDG_MUSIC_DIR`,自然落 `~/Music`。
///
/// # Return:
///   解析得到的目录路径。
pub(crate) fn music_dir() -> color_eyre::Result<PathBuf> {
    if let Some(v) = std::env::var_os("XDG_MUSIC_DIR").filter(|v| !v.is_empty())
        && Path::new(&v).is_absolute()
    {
        return Ok(PathBuf::from(v));
    }
    Ok(home_dir()?.join("Music"))
}

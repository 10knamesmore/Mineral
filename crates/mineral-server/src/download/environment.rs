//! Permanent export directory selection and download hook access.

use std::path::{Path, PathBuf};

/// 解析下载环境:永久导出根目录。不可用时为 `None`(下载整体降级为「不可用」,
/// 只 warn 不阻断启动)。
///
/// 导出目录优先级:config(`download.dir`)> 平台默认(`~/Music/mineral`)。
/// config.lua 是唯一用户真相源,不设环境变量逃逸口。
///
/// # Params:
///   - `config_dir`: 配置的下载目录(`download.dir`;`None` = 未配置)
///
/// # Return:
///   导出根目录;解析失败为 `None`。
pub(crate) fn open_env(config_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(d) = config_dir {
        Some(d.to_path_buf())
    } else {
        match mineral_paths::music_export_dir() {
            Ok(d) => Some(d),
            Err(e) => {
                mineral_log::warn!(target: "download", error = mineral_log::chain(&e), "解析音乐导出目录失败,下载不可用");
                None
            }
        }
    }
}

/// 下载环境:导出根目录 + 脚本拦截门
/// (`process_target` 从 [`crate::player::PlayerCore`] 取齐,单测各自注入)。
#[derive(Clone, Copy)]
pub(crate) struct DownloadEnv<'a> {
    /// 永久导出根目录(如 `~/Music/mineral`)。
    pub(crate) music_dir: &'a Path,

    /// 脚本拦截门(`before_download`;无脚本恒放行)。
    pub(crate) hooks: &'a crate::hook_bridge::HookGate,
}

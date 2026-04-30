//! Mineral 文件日志:append 到 `<cache_dir>/mineral.log`。

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const LOG_FILE_NAME: &str = "mineral.log";

/// 写一行 `WARN` 级别日志。写盘失败被吞,不向上传播。
///
/// # Params:
///   - `context`: 失败语境(`"Netease/my_playlists"` 这类标识)
///   - `message`: 错误描述
pub fn warn(context: &str, message: &str) {
    let _ = try_write("WARN", context, message);
}

fn try_write(level: &str, context: &str, message: &str) -> io::Result<()> {
    let path = log_path().map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    writeln!(file, "[{secs}] {level} {context}: {message}")
}

fn log_path() -> color_eyre::Result<PathBuf> {
    Ok(mineral_paths::cache_dir()?.join(LOG_FILE_NAME))
}

//! 哔哩哔哩登录凭证的本地持久化。
//!
//! CLI 二维码登录成功后,把 `SESSDATA` / `bili_jct` / `DedeUserID` 写到
//! `<data_dir>/bilibili.json`;daemon 启动时用 [`load_stored`] 读回,据此构造带登录态的
//! channel(解锁高码率 / 私密收藏夹)。未登录时文件不存在,走 guest 模式。

use std::fs;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{WrapErr, eyre};
use serde::{Deserialize, Serialize};

/// 凭证文件名,放在 `mineral_paths::data_dir()` 下。
pub const CREDENTIAL_FILE: &str = "bilibili.json";

/// 序列化到磁盘的 B站登录凭证(三件套 cookie)。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredBilibiliAuth {
    /// 核心登录 cookie `SESSDATA`。
    pub sessdata: String,

    /// CSRF token cookie `bili_jct`(写操作用;只读也一并存)。
    pub bili_jct: String,

    /// 登录用户 mid(cookie `DedeUserID`);拉「我的收藏夹」用作 `up_mid`。
    pub dede_user_id: String,
}

/// 解析得到凭证文件的绝对路径(可能尚不存在)。
///
/// # Return:
///   `<data_dir>/bilibili.json` 的绝对路径。本函数不创建目录、不做存在性检查。
pub fn credential_path() -> color_eyre::Result<PathBuf> {
    Ok(mineral_paths::data_dir()?.join(CREDENTIAL_FILE))
}

/// 把凭证写到磁盘,父目录不存在时自动创建。
///
/// # Params:
///   - `auth`: 待持久化的凭证
///
/// # Return:
///   写入的路径;失败时 `Err`。
pub fn save(auth: &StoredBilibiliAuth) -> color_eyre::Result<PathBuf> {
    let path = credential_path()?;
    write_to(&path, auth)?;
    mineral_log::info!(target: "credential", path = %path.display(), mid = auth.dede_user_id, "bilibili credential saved");
    Ok(path)
}

/// 从磁盘加载凭证。
///
/// # Return:
///   - `Ok(Some(auth))`: 文件存在且解析成功
///   - `Ok(None)`: 文件不存在(尚未登录,正常)
///   - `Err(_)`: 文件存在但读/解析失败
pub fn load_stored() -> color_eyre::Result<Option<StoredBilibiliAuth>> {
    let path = credential_path()?;
    read_from(&path)
}

/// 把 `auth` 序列化成 JSON 写到 `path`,父目录不存在时自动创建。
fn write_to(path: &Path, auth: &StoredBilibiliAuth) -> color_eyre::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| eyre!("bilibili 凭证路径缺少父目录: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create credential dir failed: {}", parent.display()))?;
    let json = serde_json::to_string_pretty(auth).context("serialize bilibili auth failed")?;
    fs::write(path, json)
        .with_context(|| format!("write bilibili auth failed: {}", path.display()))?;
    Ok(())
}

/// 从 `path` 读凭证;文件不存在 → `Ok(None)`,其他 IO/解析失败 → `Err`。
fn read_from(path: &Path) -> color_eyre::Result<Option<StoredBilibiliAuth>> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let auth: StoredBilibiliAuth = serde_json::from_str(&text)
                .with_context(|| format!("parse bilibili auth failed: {}", path.display()))?;
            Ok(Some(auth))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read bilibili auth failed: {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::{StoredBilibiliAuth, read_from, write_to};

    #[test]
    fn round_trip_via_disk() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("bilibili.json");
        let auth = StoredBilibiliAuth {
            sessdata: "opaque-sessdata".to_owned(),
            bili_jct: "csrf-token".to_owned(),
            dede_user_id: "12345".to_owned(),
        };
        write_to(&path, &auth)?;
        let loaded =
            read_from(&path)?.ok_or_else(|| color_eyre::eyre::eyre!("写盘后文件应存在"))?;
        assert_eq!(loaded.sessdata, auth.sessdata);
        assert_eq!(loaded.dede_user_id, "12345");
        Ok(())
    }

    #[test]
    fn missing_file_returns_none() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("does-not-exist.json");
        assert!(read_from(&path)?.is_none());
        Ok(())
    }
}

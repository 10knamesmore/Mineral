//! 网易云登录凭证的本地持久化。
//!
//! CLI 二维码登录成功后,把 `MUSIC_U` cookie 与登录用户 `userId` 写到
//! `<data_dir>/netease.json`;TUI 启动时再用 [`load_stored`] 读回来,
//! 并据此构造 [`crate::NeteaseChannel::with_credential`]。

use std::fs;
use std::path::{Path, PathBuf};

use color_eyre::eyre::{WrapErr, eyre};
use mineral_model::UserId;
use serde::{Deserialize, Serialize};

/// 凭证文件名,放在 `mineral_paths::data_dir()` 下。
pub const CREDENTIAL_FILE: &str = "netease.json";

/// 序列化到磁盘的网易云登录凭证。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredNeteaseAuth {
    /// 网易云核心登录 cookie(`MUSIC_U` 字段值,不含 `MUSIC_U=` 前缀)。
    pub music_u: String,

    /// 登录用户的 `userId`,拉 `user_playlists` 时需要。
    pub user_id: UserId,
}

/// 解析得到凭证文件的绝对路径(可能尚不存在)。
///
/// # Return:
///   `<data_dir>/netease.json` 的绝对路径。本函数不创建目录、不做存在性检查。
pub fn credential_path() -> color_eyre::Result<PathBuf> {
    Ok(mineral_paths::data_dir()?.join(CREDENTIAL_FILE))
}

/// 把凭证写到磁盘,父目录不存在时自动创建。
///
/// # Params:
///   - `auth`: 待持久化的凭证
///
/// # Return:
///   成功返回 `Ok(())`;父目录创建失败、序列化失败或写盘失败时返回 `Err`。
pub fn save(auth: &StoredNeteaseAuth) -> color_eyre::Result<PathBuf> {
    let path = credential_path()?;
    write_to(&path, auth)?;
    Ok(path)
}

/// 从磁盘加载凭证。
///
/// # Return:
///   - `Ok(Some(auth))`: 文件存在且解析成功
///   - `Ok(None)`: 文件不存在(尚未登录,正常状态)
///   - `Err(_)`: 文件存在但读/解析失败(磁盘损坏、JSON schema 漂移等)
pub fn load_stored() -> color_eyre::Result<Option<StoredNeteaseAuth>> {
    let path = credential_path()?;
    read_from(&path)
}

fn write_to(path: &Path, auth: &StoredNeteaseAuth) -> color_eyre::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| eyre!("netease 凭证路径缺少父目录: {}", path.display()))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("create credential dir failed: {}", parent.display()))?;
    let json = serde_json::to_string_pretty(auth).context("serialize netease auth failed")?;
    fs::write(path, json)
        .with_context(|| format!("write netease auth failed: {}", path.display()))?;
    Ok(())
}

fn read_from(path: &Path) -> color_eyre::Result<Option<StoredNeteaseAuth>> {
    match fs::read_to_string(path) {
        Ok(text) => {
            let auth: StoredNeteaseAuth = serde_json::from_str(&text)
                .with_context(|| format!("parse netease auth failed: {}", path.display()))?;
            Ok(Some(auth))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("read netease auth failed: {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::{StoredNeteaseAuth, read_from, write_to};
    use mineral_model::UserId;

    #[test]
    fn round_trip_via_disk() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("netease.json");
        let auth = StoredNeteaseAuth {
            music_u: String::from("opaque-token"),
            user_id: UserId::new("12345"),
        };

        write_to(&path, &auth)?;
        let loaded = read_from(&path)?.expect("file exists after write");

        assert_eq!(loaded.music_u, auth.music_u);
        assert_eq!(loaded.user_id, auth.user_id);
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

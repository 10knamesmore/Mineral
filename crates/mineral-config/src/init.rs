//! 配置资产的文件写出能力;文件名、目录布局和内容由 CLI 与宿主决定。

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// 一个配置资产的生成结果。
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum InitOutcome {
    /// 已写入该路径。
    Written(PathBuf),

    /// 已存在,跳过以保护用户内容。
    Skipped(PathBuf),
}

impl std::fmt::Display for InitOutcome {
    /// 供 CLI 逐项打印写入结果。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Written(path) => write!(f, "wrote    {}", path.display()),
            Self::Skipped(path) => write!(f, "skipped  {} (exists)", path.display()),
        }
    }
}

/// 创建资产目录及其父目录;错误保留目标路径。
pub fn create_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|source| Error::InitIo {
        operation: "创建配置资产目录",
        path: path.to_path_buf(),
        source,
    })
}

/// 清理并重建仅含生成资产的目录;不得传入用户配置目录。
pub fn recreate_dir(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_dir_all(path).map_err(|source| Error::InitIo {
            operation: "清理生成的 Lua 元数据目录",
            path: path.to_path_buf(),
            source,
        })?;
    }
    create_dir(path)
}

/// 仅写入不存在的用户文件;不覆盖用户配置或编辑器设置。
pub fn write_if_absent(path: &Path, content: &str) -> Result<InitOutcome> {
    if path.exists() {
        Ok(InitOutcome::Skipped(path.to_path_buf()))
    } else {
        overwrite(path, content)
    }
}

/// 写入程序分发资产;失败包含目标路径与操作。
pub fn overwrite(path: &Path, content: &str) -> Result<InitOutcome> {
    std::fs::write(path, content).map_err(|source| Error::InitIo {
        operation: "写入配置资产",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(InitOutcome::Written(path.to_path_buf()))
}

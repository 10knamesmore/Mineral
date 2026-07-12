//! 本地文件系统 backend——shelf 的 MVP 唯一实现。

use std::fs::File;
use std::path::Path;

use async_trait::async_trait;
use color_eyre::eyre::WrapErr;

use super::backend::{Entry, EntryKind, ShelfReader, ShelfStorage};

/// `std::fs` 后端:string root(OS 文件系统可达路径,含已挂载的 SMB/NFS/FUSE)走它。
///
/// backend 不知道也不关心路径背后是本地盘还是挂载点——NAS 不可达等约束是扫描器的义务。
pub struct FsStorage;

#[async_trait]
impl ShelfStorage for FsStorage {
    async fn list_dir(&self, path: &Path) -> color_eyre::Result<Vec<Entry>> {
        let owned = path.to_owned();
        // fs 列目录是阻塞 IO,丢 blocking 池,不堵 async runtime 的 worker。
        tokio::task::spawn_blocking(move || read_dir_sync(&owned))
            .await
            .wrap_err("列目录 blocking task 汇合失败")?
    }

    fn open(&self, path: &Path) -> color_eyre::Result<Box<dyn ShelfReader>> {
        let file = File::open(path).wrap_err_with(|| format!("打开文件失败:{}", path.display()))?;
        Ok(Box::new(file))
    }
}

/// 同步列目录并映射成 [`Entry`](单层,不递归)。
///
/// # Params:
///   - `path`: 待列目录
///
/// # Return:
///   直接子项列表(文件 + 子目录)。
fn read_dir_sync(path: &Path) -> color_eyre::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for dirent in std::fs::read_dir(path).wrap_err_with(|| format!("读取目录失败:{}", path.display()))?
    {
        let dirent = dirent.wrap_err("读取目录项失败")?;
        let meta = dirent.metadata().wrap_err("读取目录项元数据失败")?;
        let kind = if meta.is_dir() {
            EntryKind::Dir
        } else {
            EntryKind::File
        };
        // 目录 size 无意义,只有文件报 size(len() 是已取元数据的字段访问,无 IO,eager 无害)。
        let size = meta.is_file().then_some(meta.len());
        let mtime = meta.modified().ok();
        entries.push(Entry::new(dirent.path(), kind, size, mtime));
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::{EntryKind, FsStorage, ShelfStorage};

    /// list_dir 列出直接子项:文件报 size(> 0)、子目录 size 为 None、都带 mtime;
    /// 不递归(嵌套文件不出现在结果里)。
    #[tokio::test]
    async fn list_dir_reports_files_and_dirs() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        std::fs::write(dir.path().join("song.flac"), b"FLACdata")?;
        std::fs::create_dir(dir.path().join("sub"))?;
        std::fs::write(dir.path().join("sub").join("nested.mp3"), b"deep")?;

        let mut entries = FsStorage.list_dir(dir.path()).await?;
        entries.sort_by(|a, b| a.path().cmp(b.path()));
        assert_eq!(entries.len(), 2, "只列一层:song.flac + sub,nested.mp3 不出现");

        let file = entries
            .iter()
            .find(|e| *e.kind() == EntryKind::File)
            .ok_or_else(|| color_eyre::eyre::eyre!("应有一个文件项"))?;
        assert_eq!(file.size(), &Some(8), "文件报真实字节数");
        assert!(file.mtime().is_some(), "文件带 mtime");

        let subdir = entries
            .iter()
            .find(|e| *e.kind() == EntryKind::Dir)
            .ok_or_else(|| color_eyre::eyre::eyre!("应有一个目录项"))?;
        assert_eq!(subdir.size(), &None, "目录 size 为 None,不拿 0 当哨兵");
        Ok(())
    }

    /// open 得到的读取器可读回写入的字节(探测 / 封面抽取的前提)。
    #[tokio::test]
    async fn open_reads_back_bytes() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("a.bin");
        std::fs::write(&path, b"hello shelf")?;

        let mut reader = FsStorage.open(&path)?;
        let mut buf = String::new();
        reader.read_to_string(&mut buf)?;
        assert_eq!(buf, "hello shelf");
        Ok(())
    }

}

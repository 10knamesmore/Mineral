//! stream-download 的持久文件存储后端:把下载字节写到**指定且 drop 不删**的路径。
//!
//! 与内置 `TempStorageProvider`(`NamedTempFile`,drop 自删)的唯一区别是用普通
//! [`std::fs::File`]——播完后文件仍在,供上层把它收编进缓存。reader / writer 是同一文件
//! 的两个独立句柄(各自维护偏移),与 `TempStorageProvider` 的 `reopen` 模式一致。

use std::fs::File;
use std::io::{self, BufReader};
use std::path::PathBuf;

use stream_download::storage::StorageProvider;

/// 把 stream-download 的字节落到固定路径的存储后端。
pub(crate) struct FileStorageProvider {
    /// 目标文件路径(create 时截断;父目录自动创建)。
    path: PathBuf,
}

impl FileStorageProvider {
    /// 指定落盘路径构造。
    ///
    /// # Params:
    ///   - `path`: 目标文件路径
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl StorageProvider for FileStorageProvider {
    type Reader = BufReader<File>;

    type Writer = File;

    fn into_reader_writer(
        self,
        _content_length: Option<u64>,
    ) -> io::Result<(Self::Reader, Self::Writer)> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // writer 先 create(截断/新建),reader 再 open —— 两个独立句柄,偏移互不干扰。
        let writer = File::create(&self.path)?;
        let reader = File::open(&self.path)?;
        Ok((BufReader::new(reader), writer))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom, Write};

    use stream_download::storage::StorageProvider;

    use super::FileStorageProvider;

    /// round-trip:writer 写入的字节,reader(独立句柄)能读回;drop 后文件仍在。
    #[test]
    fn writes_persist_and_reader_sees_them() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let path = d.path().join("sub/audio.flac");
        let (mut reader, mut writer) =
            FileStorageProvider::new(path.clone()).into_reader_writer(Some(5))?;
        writer.write_all(b"hello")?;
        writer.flush()?;
        // reader 是独立句柄,从头读。
        reader.seek(SeekFrom::Start(0))?;
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        assert_eq!(buf, b"hello");
        drop((reader, writer));
        assert!(path.is_file(), "drop 后文件应仍在(非临时自删)");
        assert_eq!(std::fs::read(&path)?, b"hello");
        Ok(())
    }
}

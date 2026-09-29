//! 创建并清理 Kitty 图片使用的 POSIX shared memory object。

use std::num::NonZeroUsize;
use std::os::fd::OwnedFd;

use nix::fcntl::OFlag;
use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap, shm_open, shm_unlink};
use nix::sys::stat::Mode;
use nix::unistd::ftruncate;

/// 创建或填充 Kitty 共享内存失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// 像素缓冲区为空。
    #[error("kitty shared memory payload is empty")]
    EmptyPayload,

    /// 像素缓冲区长度无法用于共享内存。
    #[error("convert kitty payload size")]
    Size(#[from] std::num::TryFromIntError),

    /// 创建共享内存对象失败。
    #[error("create kitty shared memory {name}")]
    Create {
        /// 共享内存对象名称。
        name: String,

        /// POSIX 错误。
        #[source]
        source: nix::errno::Errno,
    },

    /// 设定对象长度失败。
    #[error("size kitty shared memory")]
    Truncate(#[source] nix::errno::Errno),

    /// 映射对象失败。
    #[error("map kitty shared memory")]
    Map(#[source] nix::errno::Errno),

    /// 解除对象映射失败。
    #[error("unmap kitty shared memory")]
    Unmap(#[source] nix::errno::Errno),
}

/// 一张 Kitty 图片对应的 POSIX shared memory 资源。
pub(super) struct SharedMemory {
    /// 传给终端的 POSIX shared memory 名称。
    name: String,

    /// 为完整 RGB / RGBA payload 预留的字节数。
    bytes: u64,
}

impl SharedMemory {
    /// 创建权限仅限当前用户的 shared memory object 并写入完整像素字节。
    ///
    /// # Params:
    ///   - `image_id`: 资源名中的 image id
    ///   - `pixels`: 与传输命令格式一致的完整 RGB8 / RGBA8 字节
    ///
    /// # Return:
    ///   保持资源生命周期的句柄
    pub(super) fn create(image_id: u32, pixels: &[u8]) -> Result<Self, Error> {
        if pixels.is_empty() {
            return Err(Error::EmptyPayload);
        }
        let bytes = u64::try_from(pixels.len())?;
        let name = format!("/mineral-{image_id}");
        let fd = shm_open(
            name.as_str(),
            OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_RDWR,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .map_err(|source| Error::Create {
            name: name.clone(),
            source,
        })?;
        let resource = Self { name, bytes };
        write_shared_memory(&fd, pixels)?;
        Ok(resource)
    }

    /// 返回传给 Kitty 命令的 shared memory 名称。
    pub(super) fn name(&self) -> &str {
        &self.name
    }

    /// 返回 shared memory payload 的预算占用。
    pub(super) const fn resident_bytes(&self) -> u64 {
        self.bytes
    }
}

/// 将完整字节 payload 写入 POSIX shared memory 映射。
///
/// # Params:
///   - `fd`: 已创建的 shared memory object descriptor
///   - `bytes`: 要写入的完整 payload
///
/// # Return:
///   写入并解除映射成功时返回 `Ok(())`
#[allow(unsafe_code)]
fn write_shared_memory(fd: &OwnedFd, bytes: &[u8]) -> Result<(), Error> {
    let length = NonZeroUsize::new(bytes.len()).ok_or(Error::EmptyPayload)?;
    let file_length = i64::try_from(bytes.len())?;
    ftruncate(fd, file_length).map_err(Error::Truncate)?;

    // SAFETY: ftruncate makes the object exactly `bytes.len()` bytes long. mmap returns a valid
    // mapping of that same non-zero length, which this function alone writes before unmapping it.
    unsafe {
        let address = mmap(
            None,
            length,
            ProtFlags::PROT_WRITE,
            MapFlags::MAP_SHARED,
            fd,
            0,
        )
        .map_err(Error::Map)?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), address.as_ptr().cast::<u8>(), bytes.len());
        munmap(address, bytes.len()).map_err(Error::Unmap)?;
    }

    Ok(())
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        let _ = shm_unlink(self.name.as_str());
    }
}

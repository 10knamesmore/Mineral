//! 创建并清理 Kitty 图片使用的 POSIX shared memory object。

use std::num::NonZeroUsize;
use std::os::fd::OwnedFd;

use color_eyre::eyre::WrapErr as _;
use nix::fcntl::OFlag;
use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap, shm_open, shm_unlink};
use nix::sys::stat::Mode;
use nix::unistd::ftruncate;

/// 一张 Kitty 图片对应的 POSIX shared memory 资源。
pub(super) struct SharedMemory {
    /// 传给终端的 POSIX shared memory 名称。
    name: String,

    /// 为完整 RGBA payload 预留的字节数。
    bytes: u64,
}

impl SharedMemory {
    /// 创建权限仅限当前用户的 shared memory object 并写入完整 RGBA8 字节。
    ///
    /// # Params:
    ///   - `image_id`: 资源名中的 image id
    ///   - `rgba`: 完整 RGBA8 字节
    ///
    /// # Return:
    ///   保持资源生命周期的句柄
    pub(super) fn create(image_id: u32, rgba: &[u8]) -> color_eyre::Result<Self> {
        let name = format!("/mineral-{image_id}");
        let fd = shm_open(
            name.as_str(),
            OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_RDWR,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .wrap_err_with(|| format!("create kitty shared memory {name}"))?;
        let bytes = u64::try_from(rgba.len()).wrap_err("convert kitty payload size")?;
        let resource = Self { name, bytes };
        write_shared_memory(&fd, rgba)?;
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
fn write_shared_memory(fd: &OwnedFd, bytes: &[u8]) -> color_eyre::Result<()> {
    let length = NonZeroUsize::new(bytes.len())
        .ok_or_else(|| color_eyre::eyre::eyre!("kitty shared memory payload is empty"))?;
    let file_length = i64::try_from(bytes.len()).wrap_err("convert kitty shared memory length")?;
    ftruncate(fd, file_length).wrap_err("size kitty shared memory")?;

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
        .wrap_err("map kitty shared memory")?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), address.as_ptr().cast::<u8>(), bytes.len());
        munmap(address, bytes.len()).wrap_err("unmap kitty shared memory")?;
    }

    Ok(())
}

impl Drop for SharedMemory {
    fn drop(&mut self) {
        let _ = shm_unlink(self.name.as_str());
    }
}

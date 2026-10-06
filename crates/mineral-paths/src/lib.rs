//! Mineral 跨平台路径解析。
//!
//! config / data / cache / 音乐目录在所有 Unix(含 **macOS**)统一走 XDG；
//! runtime / IPC socket 因平台差异(macOS 无 `XDG_RUNTIME_DIR`)单独处理。

#[cfg(windows)]
compile_error!("Windows 暂不支持");

use std::path::PathBuf;

mod socket;
mod xdg;

/// 路径解析、runtime 目录安全检查与 socket 路径校验失败。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 回退目录所需的 HOME 环境变量不存在。
    #[error("HOME 未设置，无法确定 mineral 目录")]
    MissingHome,

    /// 创建或检查 runtime 目录失败。
    #[error("{operation} runtime 目录失败: {}", .path.display())]
    RuntimeDirectory {
        /// 失败的操作。
        operation: &'static str,

        /// 目标目录。
        path: PathBuf,

        /// 原始文件系统错误。
        #[source]
        source: std::io::Error,
    },

    /// runtime 目录由另一位用户持有。
    #[error("runtime 目录属主非本人(uid={owner} != {current}),拒绝使用 {}", .path.display())]
    RuntimeOwner {
        /// 目录路径。
        path: PathBuf,

        /// 实际属主 UID。
        owner: u32,

        /// 当前进程 UID。
        current: u32,
    },

    /// Unix socket 地址超过平台的 sun_path 限制。
    #[error("socket 路径过长({length} >= {maximum}),请用 $MINERAL_SOCKET_DIR 指一个更短的目录: {}", .path.display())]
    SocketPathTooLong {
        /// 路径字节数。
        length: usize,

        /// 平台允许的最大字节数(含结尾 NUL)。
        maximum: usize,

        /// socket 路径。
        path: PathBuf,
    },
}

/// 本 crate 路径操作的结果。
pub type Result<T> = std::result::Result<T, Error>;

/// 配置根目录(`$XDG_CONFIG_HOME/mineral` 或 `~/.config/mineral`)。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn config_dir() -> Result<PathBuf> {
    xdg::config_dir()
}

/// 数据根目录(`$XDG_DATA_HOME/mineral` 或 `~/.local/share/mineral`)。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn data_dir() -> Result<PathBuf> {
    xdg::data_dir()
}

/// 缓存根目录(`$XDG_CACHE_HOME/mineral` 或 `~/.cache/mineral`)。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn cache_dir() -> Result<PathBuf> {
    xdg::cache_dir()
}

/// 音频缓存目录(`<cache_dir>/audio`)。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn audio_cache_dir() -> Result<PathBuf> {
    Ok(cache_dir()?.join("audio"))
}

/// 封面缓存目录(`<cache_dir>/cover`)。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn cover_cache_dir() -> Result<PathBuf> {
    Ok(cache_dir()?.join("cover"))
}

/// 下载导出目录的**平台默认**:`<music_dir>/mineral`(music_dir =
/// `$XDG_MUSIC_DIR` 或 `~/Music`)。永久保存的「下载的音乐」落这里,
/// 可被其他播放器 / 文件管理器直接使用,**不**受缓存 LRU 驱逐。
///
/// 用户改目录走 `daemon.lua` 的 `download.dir`(单一真相源,由 server
/// 在本默认值之上覆盖),不设环境变量逃逸口。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn music_export_dir() -> Result<PathBuf> {
    Ok(xdg::music_dir()?.join("mineral"))
}

/// 客户端(TUI)持久化数据库文件(`<data_dir>/tui.db`)。
///
/// 与 server 的 `mineral.db` 同目录的另一个 sqlite 文件,存封面缓存索引、UI 偏好和
/// 歌单曲目位置等客户端态。数据库放 data_dir(持久),封面文件本体仍落在
/// 可清理的 [`cover_cache_dir`]。
///
/// # Return:
///   解析得到的文件路径。本函数不创建目录。
pub fn tui_db() -> Result<PathBuf> {
    Ok(data_dir()?.join("tui.db"))
}

/// Runtime 目录(进程级生命周期的 ephemeral 文件,如 IPC unix socket)。
///
/// 优先级:`$MINERAL_SOCKET_DIR` → `$XDG_RUNTIME_DIR/mineral` → `$TMPDIR`(或 `/tmp`)`/mineral-<uid>`。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn runtime_dir() -> Result<PathBuf> {
    socket::runtime_dir()
}

/// IPC unix socket 的完整路径(`<runtime_dir>/mineral.sock`)。daemon bind、
/// client connect、stale 检测全走这一处,避免各调用方重复拼接。
///
/// 与 [`runtime_dir`] 不同,本函数**会**创建 runtime 目录(收紧 `0700` + 校验属主)
/// 并检查 `sun_path` 长度,调用方拿到路径即可直接用。
///
/// # Return:
///   `<runtime_dir>/mineral.sock` 的绝对路径;目录创建/属主校验失败或路径超长返回 `Err`。
pub fn socket_path() -> Result<PathBuf> {
    socket::socket_path()
}

#[cfg(test)]
#[allow(unsafe_code)] // reason: edition 2024 下 env::set_var/remove_var 是 unsafe,只在测试里用 EnvGuard 隔离
mod tests {
    use std::sync::Mutex;

    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 进程级 env 改动的 RAII 守卫:构造时设/清,Drop 时还原。配合 [`ENV_LOCK`] 串行化。
    struct EnvGuard {
        /// 被改的 env key。
        key: &'static str,

        /// 改前的原值(还原用)。
        prev: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &std::path::Path) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: 单测整个用 ENV_LOCK 串行化,不会跟其他线程并发改 env。
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            // SAFETY: 同 set,串行化保证。
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                // SAFETY: 同上。Drop 在 lock guard 释放前执行(变量声明顺序保证)。
                Some(v) => unsafe { std::env::set_var(self.key, v) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
    }

    fn fake_home() -> color_eyre::Result<TempDir> {
        Ok(tempfile::tempdir()?)
    }

    #[test]
    fn data_dir_uses_xdg_when_set() -> color_eyre::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = fake_home()?;
        let xdg = tmp.path().join("xdg-data");
        let _g1 = EnvGuard::set("HOME", tmp.path());
        let _g2 = EnvGuard::set("XDG_DATA_HOME", &xdg);

        assert_eq!(super::data_dir()?, xdg.join("mineral"));
        Ok(())
    }

    #[test]
    fn data_dir_falls_back_when_xdg_unset() -> color_eyre::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = fake_home()?;
        let _g1 = EnvGuard::set("HOME", tmp.path());
        let _g2 = EnvGuard::unset("XDG_DATA_HOME");

        assert_eq!(super::data_dir()?, tmp.path().join(".local/share/mineral"));
        Ok(())
    }

    /// 相对路径的 `$XDG_*_HOME` 按 XDG 规范当未设(必须绝对)。
    #[test]
    fn xdg_ignores_relative_value() -> color_eyre::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = fake_home()?;
        let _g1 = EnvGuard::set("HOME", tmp.path());
        // 相对路径:应被忽略,退回 $HOME/.config。
        let _g2 = EnvGuard::set("XDG_CONFIG_HOME", std::path::Path::new("relative/dir"));

        assert_eq!(super::config_dir()?, tmp.path().join(".config/mineral"));
        Ok(())
    }

    /// macOS 同样走 XDG:config 落 `~/.config/mineral`(非 `~/Library/...`)。
    #[test]
    fn config_falls_back_to_dot_config() -> color_eyre::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = fake_home()?;
        let _g1 = EnvGuard::set("HOME", tmp.path());
        let _g2 = EnvGuard::unset("XDG_CONFIG_HOME");

        assert_eq!(super::config_dir()?, tmp.path().join(".config/mineral"));
        Ok(())
    }

    /// `$MINERAL_SOCKET_DIR` 显式覆盖:socket 落 `<它>/mineral.sock`,且目录被创建。
    #[test]
    fn socket_path_honors_explicit_override() -> color_eyre::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = fake_home()?;
        let sock_dir = tmp.path().join("sock");
        let _g = EnvGuard::set("MINERAL_SOCKET_DIR", &sock_dir);

        let sock = super::socket_path()?;
        assert_eq!(sock, sock_dir.join("mineral.sock"));
        assert!(sock_dir.is_dir(), "socket_path 应创建 runtime 目录");
        Ok(())
    }

    /// socket 路径超 `sun_path` 上限 → 报错而非截断。
    #[test]
    fn socket_path_rejects_overlong() -> color_eyre::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = fake_home()?;
        // 造一个必然超 104/108 字节的深目录(绝对路径)。
        let deep = tmp.path().join("x".repeat(200));
        let _g = EnvGuard::set("MINERAL_SOCKET_DIR", &deep);

        assert!(
            matches!(
                super::socket_path(),
                Err(super::Error::SocketPathTooLong { .. })
            ),
            "超长 socket 路径应报分类错误"
        );
        Ok(())
    }

    /// HOME 缺失是明确的路径错误类别。
    #[test]
    fn missing_home_is_classified() {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _home = EnvGuard::unset("HOME");
        let _xdg = EnvGuard::unset("XDG_CONFIG_HOME");
        assert!(matches!(
            super::config_dir(),
            Err(super::Error::MissingHome)
        ));
    }

    /// 平台默认落 `<music_dir>/mineral`(music_dir 缺 XDG_MUSIC_DIR → `~/Music`);
    /// 用户覆盖走 config(`download.dir`),本函数不认任何环境变量。
    #[test]
    fn music_export_dir_falls_back_to_music_subdir() -> color_eyre::Result<()> {
        let _lock = ENV_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = fake_home()?;
        let _g1 = EnvGuard::set("HOME", tmp.path());
        let _g2 = EnvGuard::unset("XDG_MUSIC_DIR");

        assert_eq!(super::music_export_dir()?, tmp.path().join("Music/mineral"));
        Ok(())
    }
}

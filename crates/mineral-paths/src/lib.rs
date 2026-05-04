//! Mineral 跨平台路径解析。

#[cfg(windows)]
compile_error!("Windows 暂不支持");

use std::path::PathBuf;

#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(target_os = "macos", path = "macos.rs")]
mod platform;

/// 配置根目录（`$XDG_CONFIG_HOME/mineral` 或 `~/.config/mineral`）。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn config_dir() -> color_eyre::Result<PathBuf> {
    platform::config_dir()
}

/// 数据根目录（`$XDG_DATA_HOME/mineral` 或 `~/.local/share/mineral`）。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn data_dir() -> color_eyre::Result<PathBuf> {
    platform::data_dir()
}

/// 缓存根目录（`$XDG_CACHE_HOME/mineral` 或 `~/.cache/mineral`）。
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn cache_dir() -> color_eyre::Result<PathBuf> {
    platform::cache_dir()
}

/// Runtime 目录(进程级生命周期的 ephemeral 文件，如 IPC unix socket)。
///
/// - **Linux**:`$XDG_RUNTIME_DIR/mineral`,缺时 `/tmp/mineral`
/// - **macOS**:`$TMPDIR/mineral`,缺时 `/tmp/mineral`
///
/// # Return:
///   解析得到的目录路径。本函数不创建目录。
pub fn runtime_dir() -> color_eyre::Result<PathBuf> {
    platform::runtime_dir()
}

#[cfg(test)]
#[allow(unsafe_code)] // reason: edition 2024 下 env::set_var/remove_var 是 unsafe,只在测试里用 EnvGuard 隔离
mod tests {
    use std::sync::Mutex;

    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
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
}

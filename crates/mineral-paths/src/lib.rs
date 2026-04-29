//! Mineral 跨平台路径解析。
//!
//! 仅暴露三个根目录：[`config_dir`] / [`data_dir`] / [`cache_dir`]，按 XDG 语义。
//! 各个具体模块（如 channel 凭证）自行在这三个根之上 join 自己的相对路径——本 crate
//! 不耦合任何业务命名。
//!
//! 实现按 OS 分文件（`linux.rs` / `macos.rs`），目前两个平台都走 XDG，但保留 dispatch
//! 结构以便将来分叉。Windows 在编译期被拒绝。

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

#[cfg(test)]
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
            std::env::set_var(key, value);
            Self { key, prev }
        }

        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn fake_home() -> color_eyre::Result<TempDir> {
        Ok(tempfile::tempdir()?)
    }

    #[test]
    fn data_dir_uses_xdg_when_set() -> color_eyre::Result<()> {
        let _lock = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = fake_home()?;
        let xdg = tmp.path().join("xdg-data");
        let _g1 = EnvGuard::set("HOME", tmp.path());
        let _g2 = EnvGuard::set("XDG_DATA_HOME", &xdg);

        assert_eq!(super::data_dir()?, xdg.join("mineral"));
        Ok(())
    }

    #[test]
    fn data_dir_falls_back_when_xdg_unset() -> color_eyre::Result<()> {
        let _lock = ENV_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let tmp = fake_home()?;
        let _g1 = EnvGuard::set("HOME", tmp.path());
        let _g2 = EnvGuard::unset("XDG_DATA_HOME");

        assert_eq!(super::data_dir()?, tmp.path().join(".local/share/mineral"));
        Ok(())
    }
}

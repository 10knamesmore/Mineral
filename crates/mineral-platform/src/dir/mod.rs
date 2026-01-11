//! Platform-specific directories for this project.
//!
//! Simplified from `dirs-rs`, tailored for the `mineral` project.
//! Only supports desktop platforms.

#![deny(missing_docs)]

use std::path::PathBuf;

/* =========================
 * Platform dispatch
 * ========================= */

#[cfg(target_os = "windows")]
mod win;
#[cfg(target_os = "windows")]
use win as sys;

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
use mac as sys;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux as sys;

const APP_NAME: &str = "mineral";

/* =========================
 * Project directories
 * ========================= */

/// | Platform | Value           | Example                     |
/// | -------- | --------------- | --------------------------- |
/// | Linux    | `$HOME`         | `/home/wanger`       |
/// | macOS    | `$HOME`         | `/Users/wanger`      |
/// | Windows  | `%USERPROFILE%` | `C:\Users\wanger`    |
pub fn home_dir() -> PathBuf {
    sys::home_dir()
}

/// | Platform | Value                               | Example                                   |
/// | -------- | ----------------------------------- | ----------------------------------------- |
/// | Linux    | `$HOME/.cache`                      | `/home/wanger/.cache/mineral`              |
/// | macOS    | `$HOME/.cache`                      | `/Users/wanger/.cache/mineral`             |
/// | Windows  | `%LOCALAPPDATA%`                    | `C:\Users\wanger\AppData\Local\mineral`    |
pub fn cache_dir() -> PathBuf {
    sys::cache_dir().join(APP_NAME)
}

/// | Platform | Value                                 | Example                                      |
/// | -------- | ------------------------------------- | -------------------------------------------- |
/// | Linux    | `$HOME/.config`                       | `/home/wanger/.config/mineral`                |
/// | macOS    | `$HOME/.config`                       | `/Users/wanger/.config/mineral`               |
/// | Windows  | `%APPDATA%`                           | `C:\Users\wanger\AppData\Roaming\mineral`     |
pub fn config_dir() -> PathBuf {
    sys::config_dir().join(APP_NAME)
}

/// | Platform | Value                                    | Example                                          |
/// | -------- | ---------------------------------------- | ------------------------------------------------ |
/// | Linux    | `$HOME/.local/share`                     | `/home/wanger/.local/share/mineral`               |
/// | macOS    | `$HOME/.local/share`                     | `/Users/wanger/.local/share/mineral`              |
/// | Windows  | `%LOCALAPPDATA%`                         | `C:\Users\wanger\AppData\Local\mineral`           |
pub fn data_dir() -> PathBuf {
    sys::data_dir().join(APP_NAME)
}

/// | Platform | Value                                     | Example                                           |
/// | -------- | ----------------------------------------- | ------------------------------------------------- |
/// | Linux    | `$HOME/.local/state`                      | `/home/wanger/.local/state/mineral`                |
/// | macOS    | `$HOME/.local/state`                      | `/Users/wanger/.local/state/mineral`               |
/// | Windows  | `%LOCALAPPDATA%`                          | `C:\Users\wanger\AppData\Local\mineral`            |
pub fn state_dir() -> PathBuf {
    sys::state_dir().join(APP_NAME)
}

/// | Platform | Value                                     | Example                                                 |
/// | -------- | ----------------------------------------- | ------------------------------------------------------- |
/// | Linux    | `$HOME/.local/state`                      | `/home/wanger/.local/state/mineral/logs`                |
/// | macOS    | `$HOME/.local/state`                      | `/Users/wanger/.local/state/mineral/logs`               |
/// | Windows  | `%LOCALAPPDATA%`                          | `C:\Users\wanger\AppData\Local\mineral\logs`            |
pub fn logs_dir() -> PathBuf {
    state_dir().join("logs")
}

/// | Platform | Value                 | Example                            |
/// | -------- | --------------------- | ---------------------------------- |
/// | Linux    | `$HOME/Music`         | `/home/wanger/Music/mineral`        |
/// | macOS    | `$HOME/Music`         | `/Users/wanger/Music/mineral`       |
/// | Windows  | `%USERPROFILE%\Music` | `C:\Users\wanger\Music\mineral`     |
pub fn audio_dir() -> PathBuf {
    sys::audio_dir().join(APP_NAME)
}

/// | Platform | Value                     | Example                                |
/// | -------- | ------------------------- | -------------------------------------- |
/// | Linux    | `$HOME/Downloads`         | `/home/wanger/Downloads/mineral`        |
/// | macOS    | `$HOME/Downloads`         | `/Users/wanger/Downloads/mineral`       |
/// | Windows  | `%USERPROFILE%\Downloads` | `C:\Users\wanger\Downloads\mineral`     |
pub fn download_dir() -> PathBuf {
    sys::download_dir().join(APP_NAME)
}

/* =========================
 * Tests
 * ========================= */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dirs() {
        println!("home_dir:     {:?}", home_dir());
        println!();
        println!("cache_dir:    {:?}", cache_dir());
        println!("config_dir:   {:?}", config_dir());
        println!("data_dir:     {:?}", data_dir());
        println!("state_dir:    {:?}", state_dir());
        println!();
        println!("audio_dir:    {:?}", audio_dir());
        println!("download_dir: {:?}", download_dir());
    }
}

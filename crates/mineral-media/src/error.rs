//! System media service startup and delivery failures.

use thiserror::Error;

/// Failures while starting or updating the operating system media service.
#[derive(Debug, Error)]
pub enum Error {
    /// The dedicated media service thread could not start.
    #[error("spawn media service thread")]
    SpawnThread {
        /// Operating system thread creation failure.
        #[source]
        source: std::io::Error,
    },

    /// The Linux MPRIS runtime could not start.
    #[cfg(target_os = "linux")]
    #[error("build MPRIS runtime")]
    Runtime {
        /// Tokio runtime construction failure.
        #[source]
        source: std::io::Error,
    },

    /// The MPRIS player could not register with D-Bus.
    #[cfg(target_os = "linux")]
    #[error("register MPRIS player")]
    RegisterPlayer {
        /// D-Bus registration failure.
        #[source]
        source: mpris_server::zbus::Error,
    },

    /// The dedicated service thread exited before reporting readiness.
    #[cfg(target_os = "linux")]
    #[error("media service exited before ready")]
    StartupInterrupted {
        /// Startup channel disconnection.
        #[source]
        source: std::sync::mpsc::RecvError,
    },

    /// The media service thread no longer receives updates.
    #[error("media service thread is unavailable")]
    ServiceStopped,

    /// macOS media integration was initialized off the process main thread.
    #[cfg(target_os = "macos")]
    #[error("macOS media service must initialize NSApplication on the main thread")]
    NotMainThread,
}

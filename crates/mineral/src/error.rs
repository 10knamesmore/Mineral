//! 进程启动和 daemon 装配的失败原因。

/// 启动运行时、来源适配器或客户端失败，保留对应模块的原始错误。
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// 无法建立异步运行时。
    #[error("create async runtime")]
    Runtime(#[source] std::io::Error),

    /// 无法启动承载 daemon 的后台线程。
    #[cfg(target_os = "macos")]
    #[error("spawn daemon host thread")]
    DaemonThread(#[source] std::io::Error),

    /// 系统媒体集成初始化失败。
    #[cfg(target_os = "macos")]
    #[error(transparent)]
    Media(#[from] mineral_media::Error),

    /// 无法定位应用目录。
    #[error(transparent)]
    Paths(#[from] mineral_paths::Error),

    /// 内置配置或配置运行环境无法加载。
    #[error(transparent)]
    Config(#[from] mineral_config::Error),

    /// 命令行或 daemon 服务启动失败。
    #[error(transparent)]
    Cli(Box<mineral_cli::Error>),

    /// 终端客户端启动或运行失败。
    #[error(transparent)]
    Tui(#[from] mineral_tui::Error),

    /// 播放资源提供者无法注册。
    #[error(transparent)]
    Playback(#[from] mineral_playback::Error),

    /// 网易云凭证读取或适配器初始化失败。
    #[error(transparent)]
    Netease(#[from] mineral_channel_netease::Error),

    /// B站凭证读取或适配器初始化失败。
    #[error(transparent)]
    Bilibili(#[from] mineral_channel_bilibili::Error),
}

/// 在进程装配边界装箱较大的 CLI 错误，保留其类型和原因链。
impl From<mineral_cli::Error> for Error {
    fn from(source: mineral_cli::Error) -> Self {
        Self::Cli(Box::new(source))
    }
}

/// 进程装配步骤的结果。
pub(crate) type Result<T> = std::result::Result<T, Error>;

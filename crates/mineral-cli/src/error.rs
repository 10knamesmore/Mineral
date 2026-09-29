//! CLI 命令执行失败的领域边界。

use std::path::PathBuf;

/// 命令执行失败，保留操作目标和底层原因供进程入口诊断。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 无法创建命令所需的异步运行时。
    #[error("create CLI runtime")]
    Runtime(#[source] std::io::Error),

    /// 命令被送进不支持它的分发入口。
    #[error("command dispatched to the wrong entry point")]
    InvalidDispatch,

    /// 无法定位应用目录或 socket。
    #[error(transparent)]
    Paths(#[from] mineral_paths::Error),

    /// 无法加载配置。
    #[error(transparent)]
    Config(#[from] mineral_config::Error),

    /// 缓存或用户数据存储失败。
    #[error(transparent)]
    Persist(#[from] mineral_persist::Error),

    /// 统计数据存储失败。
    #[error(transparent)]
    Stats(#[from] mineral_stats::Error),

    /// 网易云 channel 命令失败。
    #[error(transparent)]
    Netease(#[from] mineral_channel_netease::Error),

    /// B 站 channel 命令失败。
    #[error(transparent)]
    Bilibili(#[from] mineral_channel_bilibili::Error),

    /// daemon 服务失败。
    #[error(transparent)]
    Server(#[from] mineral_server::Error),

    /// 连接 daemon socket 失败。
    #[error("connect to daemon at {} failed", path.display())]
    SocketConnect {
        /// daemon socket 路径。
        path: PathBuf,

        /// 底层传输错误。
        #[source]
        source: mineral_protocol::WireError,
    },

    /// 指定 socket 的会话握手失败。
    #[error("handshake with daemon at {} failed", path.display())]
    Handshake {
        /// daemon socket 路径。
        path: PathBuf,

        /// 原始握手错误。
        #[source]
        source: mineral_client::connection::ConnectError,
    },

    /// 远程操作被拒绝。
    #[error("daemon rejected {operation}: {kind:?}")]
    Rejected {
        /// 被拒绝的操作。
        operation: &'static str,

        /// 协议失败分类。
        kind: mineral_protocol::FailureKind,
    },

    /// 会话关闭前未能确认远程操作结论。
    #[error("daemon result unknown for {operation}")]
    UnknownOutcome {
        /// 未确认的操作。
        operation: &'static str,

        /// client 给出的未提交或应答丢失原因。
        #[source]
        reason: mineral_client::operation::UnknownReason,
    },

    /// 文件系统操作失败。
    #[error("{operation} {} failed", path.display())]
    Io {
        /// 失败的操作。
        operation: &'static str,

        /// 目标路径。
        path: PathBuf,

        /// 原始 I/O 失败。
        #[source]
        source: std::io::Error,
    },

    /// 操作系统信号处理失败。
    #[error("{operation} failed")]
    Signal {
        /// 信号操作。
        operation: &'static str,

        /// 原始 I/O 失败。
        #[source]
        source: std::io::Error,
    },

    /// 指定路径已经有 daemon 在监听。
    #[error("daemon already running at {}", path.display())]
    DaemonRunning {
        /// 正在使用的 socket。
        path: PathBuf,
    },

    /// 已连接 socket 无法提供对端进程 ID。
    #[error("daemon socket peer PID unavailable")]
    MissingPeerPid,

    /// 发送停机信号失败。
    #[error("send SIGTERM to daemon pid {pid} failed")]
    Kill {
        /// 从 socket 凭据读取的进程 ID。
        pid: i32,

        /// 原始系统调用错误。
        #[source]
        source: nix::errno::Errno,
    },

    /// 等待 daemon 退出超时。
    #[error("daemon did not exit within {timeout:?}; socket {} still present", path.display())]
    ExitTimeout {
        /// 停机等待期限。
        timeout: std::time::Duration,

        /// 尚未删除的 socket。
        path: PathBuf,
    },

    /// stats 时间窗口不合法。
    #[error(transparent)]
    Window(#[from] crate::WindowError),

    /// 系统时钟早于 Unix epoch。
    #[error("system time before Unix epoch")]
    Clock(#[source] std::time::SystemTimeError),

    /// 数值无法表示为统计毫秒或配置选项。
    #[error("{field} exceeds supported range")]
    NumberOverflow {
        /// 溢出的字段。
        field: &'static str,

        /// 原始数值转换错误。
        #[source]
        source: std::num::TryFromIntError,
    },

    /// JSON 输出无法编码。
    #[error("serialize {output} JSON failed")]
    Json {
        /// 失败的输出种类。
        output: &'static str,

        /// 原始序列化错误。
        #[source]
        source: serde_json::Error,
    },
}

/// CLI 命令执行结果。
pub type Result<T> = std::result::Result<T, Error>;

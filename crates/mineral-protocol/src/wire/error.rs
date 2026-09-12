//! 传输失败的原因及失败操作，保留可追溯的底层错误。

use thiserror::Error;

/// 传输失败：区分对端断开、底层 I/O 失败和消息编解码失败。
#[derive(Debug, Error)]
pub enum WireError {
    /// 对端已关闭，无法继续发送消息。
    #[error("传输对端已关闭")]
    Disconnected,

    /// 底层 I/O 失败，调用方可读取原始错误类别决定如何恢复。
    #[error("{operation}: {source:#}")]
    Io {
        /// 失败的连接、读取、写入或关闭操作。
        operation: &'static str,

        /// 原始 I/O 错误。
        source: std::io::Error,
    },

    /// 消息无法编码或解码，保留完整诊断链。
    #[error("{operation}: {source:#}")]
    Protocol {
        /// 失败的消息编解码操作。
        operation: &'static str,

        /// 编解码错误及其上下文。
        source: color_eyre::Report,
    },
}

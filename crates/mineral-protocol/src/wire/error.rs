//! 传输失败的原因及失败操作，保留可追溯的底层错误。

use thiserror::Error;

use crate::CodecError;

/// 传输失败：区分对端断开、底层 I/O 失败和消息编解码失败。
#[derive(Debug, Error)]
pub enum WireError {
    /// 对端已关闭，无法继续发送消息。
    #[error("传输对端已关闭")]
    Disconnected,

    /// 底层 I/O 失败，调用方可读取原始错误类别决定如何恢复。
    #[error("{operation}失败")]
    Io {
        /// 失败的连接、读取、写入或关闭操作。
        operation: &'static str,

        /// 原始 I/O 错误。
        source: std::io::Error,
    },

    /// 消息无法编码、解码或帧长超限，保留具体编解码错误。
    #[error("{operation}失败")]
    Protocol {
        /// 失败的消息编解码操作。
        operation: &'static str,

        /// 具体编解码或帧解析错误。
        source: CodecError,
    },
}

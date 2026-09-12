//! 结构化会话消息的双向收发接口，支持拆成独立读写两半。

use async_trait::async_trait;

use crate::MessageBatch;

use super::WireError;

/// 发送半:writer task 独占驱动。
#[async_trait]
pub trait WireSink: Send {
    /// 发送一批消息(同方向保持调用顺序)。
    ///
    /// # Params:
    ///   - `batch`: 待发送消息
    ///
    /// # Errors
    /// 连接断开 / 底层 I/O 失败。
    async fn send(&mut self, batch: MessageBatch) -> Result<(), WireError>;

    /// 尽力关闭发送半；会话结束消息由调用方在关闭前发送。
    async fn close(&mut self) -> Result<(), WireError>;
}

/// 接收半:reader task 独占驱动。
#[async_trait]
pub trait WireSource: Send {
    /// 接收下一批消息;`Ok(None)` = 对端正常关闭。
    ///
    /// # Errors
    /// 解码失败 / 底层 I/O 失败。
    async fn recv(&mut self) -> Result<Option<MessageBatch>, WireError>;
}

/// 收发结构化消息的连接端点。
///
/// 实现方负责底层传输及必要的字节编解码，接收结束时返回 `Ok(None)`。
/// 握手阶段整体读写；握手完成后通过 [`Wire::split`] 将收发交给独立任务，
/// 使等待发送不会阻塞接收。
#[async_trait]
pub trait Wire: Send {
    /// 发送一批消息。
    ///
    /// # Params:
    ///   - `batch`: 待发送消息
    ///
    /// # Errors
    /// 连接断开 / 底层 I/O 失败。
    async fn send(&mut self, batch: MessageBatch) -> Result<(), WireError>;

    /// 接收下一批消息;`Ok(None)` = 对端关闭。
    ///
    /// # Errors
    /// 解码失败 / 底层 I/O 失败。
    async fn recv(&mut self) -> Result<Option<MessageBatch>, WireError>;

    /// 关闭承载。
    ///
    /// # Errors
    /// 底层关闭失败。
    async fn close(&mut self) -> Result<(), WireError>;

    /// 拆成独立读写两半(reader / writer 各自驱动,慢写不阻塞读)。
    #[must_use]
    fn split(self: Box<Self>) -> (Box<dyn WireSink>, Box<dyn WireSource>);
}

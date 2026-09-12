//! 供会话测试使用的内存双向传输，直接交换结构化消息并模拟有界队列背压。

use async_trait::async_trait;
use mineral_protocol::{MessageBatch, Wire, WireError, WireSink, WireSource};
use tokio::sync::mpsc;

/// 内存承载的一侧:发送进自己的 out 通道,从自己的 in 通道接收。
pub(super) struct MemoryWire {
    /// 本侧发送端(对端接收半从这里读)。
    out: mpsc::Sender<MessageBatch>,

    /// 本侧接收端。
    incoming: mpsc::Receiver<MessageBatch>,
}

impl MemoryWire {
    /// 创建一对互通的内存承载。
    ///
    /// # Params:
    ///   - `capacity`: 每个方向可积压的批次数(超出时 `send` 等待,复现真实背压)
    ///
    /// # Return:
    ///   `(client_side, server_side)` 两条互补的承载。
    #[must_use]
    pub(super) fn pair(capacity: usize) -> (Self, Self) {
        let (client_tx, server_rx) = mpsc::channel(capacity.max(1));
        let (server_tx, client_rx) = mpsc::channel(capacity.max(1));
        (
            Self {
                out: client_tx,
                incoming: client_rx,
            },
            Self {
                out: server_tx,
                incoming: server_rx,
            },
        )
    }
}

#[async_trait]
impl Wire for MemoryWire {
    async fn send(&mut self, batch: MessageBatch) -> Result<(), WireError> {
        self.out
            .send(batch)
            .await
            .map_err(|_closed| WireError::Disconnected)
    }

    async fn recv(&mut self) -> Result<Option<MessageBatch>, WireError> {
        Ok(self.incoming.recv().await)
    }

    async fn close(&mut self) -> Result<(), WireError> {
        // 内存承载没有半关闭语义:对端靠发送端被 drop 看到 None。
        Ok(())
    }

    fn split(self: Box<Self>) -> (Box<dyn WireSink>, Box<dyn WireSource>) {
        let Self { out, incoming } = *self;
        (
            Box::new(MemorySink { out }),
            Box::new(MemorySource { incoming }),
        )
    }
}

/// 内存承载的发送半。
struct MemorySink {
    /// 对端接收半。
    out: mpsc::Sender<MessageBatch>,
}

#[async_trait]
impl WireSink for MemorySink {
    async fn send(&mut self, batch: MessageBatch) -> Result<(), WireError> {
        self.out
            .send(batch)
            .await
            .map_err(|_closed| WireError::Disconnected)
    }

    async fn close(&mut self) -> Result<(), WireError> {
        // 见 [`MemoryWire::close`]:不等待对端。
        Ok(())
    }
}

/// 内存承载的接收半。
struct MemorySource {
    /// 对端发送半。
    incoming: mpsc::Receiver<MessageBatch>,
}

#[async_trait]
impl WireSource for MemorySource {
    async fn recv(&mut self) -> Result<Option<MessageBatch>, WireError> {
        Ok(self.incoming.recv().await)
    }
}

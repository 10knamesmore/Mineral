//! 在内存通道和 Unix socket 上运行相同的会话测试。

use mineral_protocol::{SocketWire, Wire};

use super::memory::MemoryWire;

/// 被测传输矩阵。
#[derive(Clone, Copy, Debug)]
pub(crate) enum TestTransport {
    /// 进程内成对通道。
    Memory,

    /// Unix socket 对。
    Socket,
}

impl TestTransport {
    /// 两种承载各一份。
    pub(crate) const ALL: [Self; 2] = [Self::Memory, Self::Socket];

    /// 造一对已连接的承载 `(client_side, server_side)`。
    ///
    /// # Params:
    ///   - `capacity`: 内存承载每方向可积压的批次数(忽略 socket)
    pub(crate) fn pair(
        self,
        capacity: usize,
    ) -> color_eyre::Result<(Box<dyn Wire>, Box<dyn Wire>)> {
        match self {
            Self::Memory => {
                let (client, server) = MemoryWire::pair(capacity);
                Ok((Box::new(client), Box::new(server)))
            }
            Self::Socket => {
                let (client, server) = tokio::net::UnixStream::pair()?;
                Ok((
                    Box::new(SocketWire::from_stream(client)),
                    Box::new(SocketWire::from_stream(server)),
                ))
            }
        }
    }
}

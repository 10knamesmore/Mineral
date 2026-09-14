//! 连接 daemon 所需的容量配置、连接错误，以及会话启动数据和运行指标。
//!
//! 建立连接时选择容量并处理连接失败；来源能力和脚本键绑定按需在连接后查询。
//! 指标记录当前会话的发送量和更新丢弃量，供调用方诊断连接。

use mineral_channel_core::ChannelCaps;
use mineral_model::SourceKind;
use mineral_protocol::{HandshakeRejected, ScriptBind, WireError};

/// 会话容量参数(默认值面向 TUI;CLI 可用 [`Self::cli`])。
#[derive(Clone, Copy, Debug)]
pub struct ClientConfig {
    /// 待发命令队列上限(满时 TUI 侧 `try_send` 立即报未提交)。
    pub outbound_capacity: usize,

    /// 单批最多合并的消息数。
    pub max_batch: usize,

    /// 同时未完成请求上限(含已入队未发送)。
    pub max_in_flight: usize,

    /// 事件缓冲上限(超出丢最旧并计数)。
    pub event_capacity: usize,

    /// PCM 近期窗口样本上限。
    pub pcm_window: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            outbound_capacity: 2048,
            max_batch: 64,
            max_in_flight: 1024,
            event_capacity: 4096,
            pcm_window: 32 * 1024,
        }
    }
}

impl ClientConfig {
    /// CLI 一次性命令用的紧凑容量。
    #[must_use]
    pub fn cli() -> Self {
        Self {
            outbound_capacity: 16,
            max_batch: 8,
            max_in_flight: 16,
            event_capacity: 64,
            pcm_window: 1024,
        }
    }
}

/// 会话建立失败。
#[derive(Debug)]
pub enum ConnectError {
    /// 传输层失败。
    Wire(WireError),

    /// 握手被拒(版本不匹配)。
    Rejected(HandshakeRejected),

    /// 协议违规(首帧不是 Welcome 等)。
    Protocol {
        /// 人读细节。
        detail: String,
    },
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Wire(e) => write!(f, "{e}"),
            Self::Rejected(e) => write!(f, "{e}"),
            Self::Protocol { detail } => f.write_str(detail),
        }
    }
}

impl std::error::Error for ConnectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Wire(source) => Some(source),
            Self::Rejected(source) => Some(source),
            Self::Protocol { .. } => None,
        }
    }
}

impl From<WireError> for ConnectError {
    fn from(value: WireError) -> Self {
        Self::Wire(value)
    }
}

/// 会话运行指标(汇总,不逐条记录)。
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionMetrics {
    /// 已发送批次数(= 每次底层 sink send 一次)。
    pub batches_sent: u64,

    /// 已发送消息数。
    pub messages_sent: u64,

    /// 已发送请求数。
    pub requests_sent: u64,

    /// 因容量丢弃的更新数。
    pub updates_dropped: u64,
}

/// 会话启动自举数据(连接后一次拉齐)。
#[derive(Clone, Debug, Default)]
pub struct Bootstrap {
    /// 各源能力声明。
    pub channel_caps: Vec<(SourceKind, ChannelCaps)>,

    /// 脚本键绑定表。
    pub script_binds: Vec<ScriptBind>,
}

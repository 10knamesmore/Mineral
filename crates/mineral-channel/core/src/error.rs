use thiserror::Error;

/// 跨 channel 边界保留具体错误类型及因果链，不依赖具体适配器 crate。
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// channel 操作可能返回的错误。
#[derive(Debug, Error)]
pub enum Error {
    /// 网络层错误(连接失败、超时等)。
    #[error("network")]
    Network {
        /// 底层连接、请求或响应读取错误。
        #[source]
        source: BoxError,
    },

    /// channel 业务层 API 返回非成功 code。
    #[error("api code {code}: {}", .message.as_deref().unwrap_or("no message provided"))]
    Api {
        /// channel 自定义的错误 code。
        code: i64,

        /// 来源响应提供的错误描述；字段缺失时为 `None`。
        message: Option<String>,
    },

    /// 当前操作需要登录。
    #[error("authentication required")]
    AuthRequired,

    /// 被服务端限流。
    #[error("rate limited")]
    RateLimited,

    /// 请求的实体不存在。
    #[error("entity not found")]
    NotFound,

    /// 该 channel 不支持此能力。
    #[error("not supported by this channel")]
    NotSupported,

    /// 响应解析或数值转换失败(JSON 结构变更、字段缺失等)。
    #[error("parse")]
    Parse {
        /// 解析器或整数转换错误，JSON 错误包含字段位置。
        #[source]
        source: BoxError,
    },

    /// 响应缺少必需字段或资源不可用。
    #[error("invalid channel data: {field}")]
    InvalidData {
        /// 缺失或不合法的字段或资源。
        field: &'static str,
    },

    /// 本地存储操作失败。
    #[error("storage")]
    Storage {
        /// 底层持久化错误。
        #[source]
        source: BoxError,
    },
}

/// channel 操作的标准 `Result` 别名。
pub type Result<T> = std::result::Result<T, Error>;

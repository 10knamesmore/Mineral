use thiserror::Error;

/// channel 操作可能返回的错误。
#[derive(Debug, Error)]
pub enum Error {
    /// 网络层错误(连接失败、超时等)。
    #[error("network: {0}")]
    Network(String),

    /// channel 业务层 API 返回非成功 code。
    #[error("api code {code}: {message}")]
    Api {
        /// channel 自定义的错误 code。
        code: i64,
        /// 错误描述。
        message: String,
    },

    /// 当前操作需要登录。
    #[error("authentication required")]
    AuthRequired,

    /// 被服务端限流。
    #[error("rate limited")]
    RateLimited,

    /// 该 channel 不支持此能力。
    #[error("not supported by this channel")]
    NotSupported,

    /// 响应解析失败(JSON 结构变更、字段缺失等)。
    #[error("parse: {0}")]
    Parse(String),

    /// 其他兜底错误。
    #[error(transparent)]
    Other(#[from] color_eyre::Report),
}

/// channel 操作的标准 `Result` 别名。
pub type Result<T> = std::result::Result<T, Error>;

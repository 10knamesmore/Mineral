//! 网易云 channel 的可追溯错误。

use std::path::PathBuf;

use thiserror::Error as ThisError;

/// 网易云请求、凭证与登录操作的错误。
#[derive(Debug, ThisError)]
pub enum Error {
    /// 远端返回非成功业务码。
    #[error(transparent)]
    Api(
        /// 远端业务码和消息。
        #[from]
        ApiCodeError,
    ),

    /// HTTP 客户端、cookie 或网络操作失败。
    #[error("{operation}")]
    Network {
        /// 失败的请求阶段。
        operation: &'static str,

        /// 底层错误。
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// JSON 解析或字段反序列化失败。
    #[error("parse {context}")]
    Parse {
        /// 响应或字段路径。
        context: String,

        /// 解析器错误。
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// JSON 请求或凭证序列化失败。
    #[error("serialize JSON")]
    Serialize(
        /// JSON 序列化错误。
        #[from]
        serde_json::Error,
    ),

    /// 必需的字段或资源缺失。
    #[error("missing or invalid {field}")]
    InvalidData {
        /// 字段或资源名。
        field: &'static str,
    },

    /// 凭证路径没有父目录。
    #[error("credential path has no parent: {}", path.display())]
    InvalidCredentialPath {
        /// 无法写入的凭证路径。
        path: PathBuf,
    },

    /// 路径解析失败。
    #[error(transparent)]
    Path(
        /// 平台路径解析错误。
        #[from]
        mineral_paths::Error,
    ),

    /// 本地凭证文件操作失败。
    #[error("{operation} {}", path.display())]
    File {
        /// 失败的文件操作。
        operation: &'static str,

        /// 文件或目录路径。
        path: PathBuf,

        /// 文件系统错误。
        #[source]
        source: std::io::Error,
    },

    /// 来源缓存持久化失败。
    #[error(transparent)]
    Storage(
        /// 来源缓存存储错误。
        #[from]
        mineral_channel_core::store::StoreError,
    ),

    /// 歌单偏移量无法在本机表示。
    #[error("playlist offset exceeds platform range")]
    PlaylistOffset(
        /// 超范围的偏移量转换错误。
        #[from]
        std::num::TryFromIntError,
    ),

    /// 二维码无法编码登录 URL。
    #[error("generate login QR")]
    Qr(
        /// 二维码编码器错误。
        #[from]
        qrcode::types::QrError,
    ),

    /// 登录成功后获取用户身份失败。
    #[error("fetch account ID after QR login")]
    LoginAccount {
        /// 获取账户信息时的原始失败。
        #[source]
        source: Box<Self>,
    },

    /// 用户取消扫码登录。
    #[error("QR login cancelled")]
    LoginCancelled,

    /// 登录二维码已失效。
    #[error("QR code expired; run login again")]
    LoginExpired,

    /// 登录接口返回未识别的状态。
    #[error("unknown QR login status code: {code}")]
    LoginStatus {
        /// 接口返回的状态码。
        code: i64,
    },
}

impl Error {
    /// 读取端点可以退避的限流业务码；写端点不启用自动重试。
    pub(crate) fn is_rate_limited(&self) -> bool {
        matches!(
            self,
            Self::Api(ApiCodeError {
                code: 405 | 512,
                ..
            })
        )
    }
}

/// 网易云操作结果。
pub type Result<T> = std::result::Result<T, Error>;

/// 网易云业务层非 200 code 的结构化错误。
#[derive(Debug, Clone, ThisError)]
#[error("api code {code}: {}", .message.as_deref().unwrap_or("no message provided"))]
pub struct ApiCodeError {
    /// 网易云返回的业务 code(301 未登录 / 512 风控或容量 / 502 已存在等)。
    pub code: i64,

    /// 服务端 `message` / `msg` 字段。
    pub message: Option<String>,
}

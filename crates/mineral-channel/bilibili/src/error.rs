//! B站 channel 的可追溯错误。

use std::path::PathBuf;

use thiserror::Error as ThisError;

/// B站请求、凭证与登录操作的错误。
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

    /// 构造或注入登录 cookie 失败。
    #[error("{operation} cookie {name}")]
    Cookie {
        /// 失败的 cookie 操作。
        operation: &'static str,

        /// 出错的 cookie 名称。
        name: &'static str,

        /// 底层 cookie 错误。
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

    /// 本机时钟无法生成 WBI 请求的 Unix 时间戳。
    #[error("read system clock for WBI signature")]
    SigningClock(#[source] std::time::SystemTimeError),

    /// WBI 签名所需的 nav key 获取失败。
    #[error("fetch WBI signing keys")]
    SigningKeys {
        /// nav 请求、解析或 key 提取时的错误。
        #[source]
        source: Box<Self>,
    },

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

    /// 凭证 JSON 序列化失败。
    #[error("serialize credential JSON")]
    Serialize(
        /// 凭证 JSON 序列化错误。
        #[from]
        serde_json::Error,
    ),

    /// 二维码无法编码登录 URL。
    #[error("generate login QR")]
    Qr(
        /// 二维码编码器错误。
        #[from]
        qrcode::types::QrError,
    ),

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

/// B站操作结果。
pub type Result<T> = std::result::Result<T, Error>;

/// B站信封 `{code, message, data}` 的非零业务错误。
#[derive(Debug, Clone, ThisError)]
#[error("api code {code}: {}", .message.as_deref().unwrap_or("no message provided"))]
pub struct ApiCodeError {
    /// B站返回的业务 code(-101 未登录 / -352 风控 / -404 不存在)。
    pub code: i64,

    /// 服务端 `message` 字段。
    pub message: Option<String>,
}

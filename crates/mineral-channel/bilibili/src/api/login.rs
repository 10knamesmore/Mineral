//! 二维码登录端点(passport 域,明文 GET)。

use serde::Deserialize;

use crate::transport::Transport;
use crate::wire::de::from_value;

/// 申请登录二维码端点。
const GENERATE_URL: &str = "https://passport.bilibili.com/x/passport-login/web/qrcode/generate";

/// 轮询登录状态端点。
const POLL_URL: &str = "https://passport.bilibili.com/x/passport-login/web/qrcode/poll";

/// 二维码生成结果:待渲染的 url + 轮询用 key。
#[derive(Debug, Clone, Deserialize)]
pub struct QrcodeGenerate {
    /// 二维码内容 URL(渲染成二维码给手机 App 扫)。
    pub url: String,

    /// 轮询登录状态用的 key。
    pub qrcode_key: String,
}

/// 轮询结果:`code` 是登录状态码(`0` 成功 / `86101` 未扫 / `86090` 已扫未确认 / `86038` 失效)。
#[derive(Debug, Clone, Deserialize)]
pub struct QrcodePoll {
    /// 登录状态码。
    pub code: i64,

    /// 状态描述。
    #[serde(default)]
    pub message: String,
}

/// 申请一个登录二维码。
///
/// # Params:
///   - `transport`: HTTP 传输层
///
/// # Return:
///   二维码 url + 轮询 key。
pub async fn generate(transport: &Transport) -> color_eyre::Result<QrcodeGenerate> {
    let data = transport.get_data(GENERATE_URL).await?;
    from_value(data)
}

/// 轮询二维码登录状态;成功(`code == 0`)时 Set-Cookie 已把凭证写进传输层 jar。
///
/// # Params:
///   - `transport`: HTTP 传输层
///   - `qrcode_key`: [`generate`] 返回的 key
///
/// # Return:
///   登录状态。
pub async fn poll(transport: &Transport, qrcode_key: &str) -> color_eyre::Result<QrcodePoll> {
    let data = transport
        .get_data(&format!("{POLL_URL}?qrcode_key={qrcode_key}"))
        .await?;
    from_value(data)
}

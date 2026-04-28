//! 登录相关端点。
//!
//! 当前覆盖:
//! - `LoginRefreshService`(`/weapi/login/token/refresh`):用 jar 里的 `MUSIC_U` 续签
//! - `LoginQRService`(`GetKey`/`CheckQR`):二维码登录

use anyhow::{anyhow, Result};
use serde_json::json;

use crate::transport::client::{RequestSpec, Transport};
use crate::transport::headers::UaKind;
use crate::transport::url::Crypto;

#[derive(Debug, Clone)]
pub struct LoginQrCode {
    pub url: String,
    pub unikey: String,
}

pub async fn login_refresh(transport: &Transport) -> Result<()> {
    transport
        .request(RequestSpec {
            path: "/weapi/login/token/refresh",
            crypto: Crypto::Weapi,
            params: serde_json::Map::new(),
            ua: UaKind::Pc,
        })
        .await
        .map(|_| ())
}

/// 调 `GetKey` 拿 unikey,拼出二维码 URL。
pub async fn login_qr_get_key(transport: &Transport) -> Result<LoginQrCode> {
    let mut p = serde_json::Map::new();
    p.insert("type".into(), json!("1"));
    p.insert("noCheckToken".into(), json!("true"));
    let v = transport
        .request(RequestSpec {
            path: "/weapi/login/qrcode/unikey",
            crypto: Crypto::Weapi,
            params: p,
            ua: UaKind::Pc,
        })
        .await?;
    let unikey = v
        .get("unikey")
        .and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("qrcode/unikey response missing `unikey`"))?
        .to_owned();
    let chain_id = crate::device::generate_chain_id();
    let url = format!(
        "http://music.163.com/login?codekey={unikey}&chainId={chain_id}",
    );
    Ok(LoginQrCode { url, unikey })
}

/// 调 `CheckQR` 轮询扫码状态。返回 code:801=等待扫码、802=待手机确认、
/// 803=登录成功(cookie 已写入 jar)、800=二维码失效。
pub async fn login_qr_check(transport: &Transport, unikey: &str) -> Result<i64> {
    let mut p = serde_json::Map::new();
    p.insert("type".into(), json!("1"));
    p.insert("noCheckToken".into(), json!("true"));
    p.insert("key".into(), json!(unikey));

    // 这个端点会用非 200 的 code 表达扫码状态(801/802/800),不能让 transport
    // 因为 code != 200 报错,所以走"宽松路径"——直接调底层。
    let result = transport
        .request_lax(RequestSpec {
            path: "/weapi/login/qrcode/client/login",
            crypto: Crypto::Weapi,
            params: p,
            ua: UaKind::Pc,
        })
        .await?;
    Ok(result
        .get("code")
        .and_then(|x| x.as_i64())
        .unwrap_or(0))
}

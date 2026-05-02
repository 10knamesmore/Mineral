use std::time::{SystemTime, UNIX_EPOCH};

use super::ids::generate_sdevice_id;

/// 二维码登录用的 ChainID。
///
/// 形如 `v1_<sDeviceId>_web_login_<unix_millis>`。
pub fn generate_chain_id() -> String {
    let s_device = generate_sdevice_id();
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("v1_{s_device}_web_login_{millis}")
}

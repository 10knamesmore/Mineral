use once_cell::sync::Lazy;
use rand::Rng;

use super::ids_data::DEVICE_ID_POOL;
use crate::crypto::constants::STD_CHARS;

/// 进程启动时随机选定的 deviceId,整个会话不变。
pub fn global_device_id() -> &'static str {
    static SELECTED: Lazy<&'static str> = Lazy::new(|| {
        let mut rng = rand::rng();
        let idx = rng.random_range(0..DEVICE_ID_POOL.len());
        DEVICE_ID_POOL[idx]
    });
    *SELECTED
}

/// 生成 `sDeviceId`,形如 `unknown-<0..1_000_000>`。
///
/// 注意每次调用会得到新值;调用方负责把它存进 cookie jar 后保持不变。
pub fn generate_sdevice_id() -> String {
    let mut rng = rand::rng();
    let n: u32 = rng.random_range(0..1_000_000);
    format!("unknown-{n}")
}

/// 生成 `_ntes_nuid` / `NMTID` 用的 16 字符随机字符串的 hex(小写)形式。
pub fn generate_ntes_nuid() -> String {
    let mut rng = rand::rng();
    let mut buf = [0u8; 16];
    for slot in &mut buf {
        let i = rng.random_range(0..STD_CHARS.len());
        *slot = STD_CHARS[i];
    }
    hex::encode(buf)
}

//! 设备指纹相关:deviceId 池、sDeviceId、ChainID 等。

/// `ChainID` 设备链标识的生成。
mod chain;
/// 生成 `deviceId` / `sDeviceId` / `ntes_nuid` 等设备指纹字段。
mod ids;
/// `ids` 用到的预置数据(从池中按规则采样的 `deviceId` 列表等)。
mod ids_data;

pub use chain::generate_chain_id;
pub use ids::{generate_ntes_nuid, generate_sdevice_id, global_device_id};

//! CPAL 输出设备枚举和按身份解析；不建立输出流。

use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait};

use crate::{Error, OutputDevice, OutputTarget, Result};

/// 返回 daemon 所在机器的输出设备；单个已拔出的设备不阻止返回其他设备。
pub(super) fn list() -> Result<Vec<OutputDevice>> {
    let host = cpal::default_host();
    let default_id = default_id();
    let mut devices = Vec::new();
    for device in host
        .output_devices()
        .map_err(|source| Error::ListDevices { source })?
    {
        match describe(&device) {
            Ok((id, name)) => devices.push(OutputDevice {
                is_default: default_id.as_ref() == Some(&id),
                id,
                name,
            }),
            Err(error) => {
                mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "output device metadata unavailable")
            }
        }
    }
    Ok(devices)
}

/// 解析一次选择；不存在的设备明确失败，不回退到扬声器。
pub(super) fn resolve(target: &OutputTarget) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match target {
        OutputTarget::SystemDefault => host.default_output_device().ok_or(Error::NoDefaultDevice),
        OutputTarget::Device(id) => {
            let parsed = id
                .parse::<cpal::DeviceId>()
                .map_err(|source| Error::InvalidDeviceId {
                    id: id.clone(),
                    source,
                })?;
            host.device_by_id(&parsed)
                .ok_or_else(|| Error::DeviceUnavailable { id: id.clone() })
        }
    }
}

/// 读取 CPAL 返回的设备 ID 与名称。
pub(super) fn describe(device: &cpal::Device) -> Result<(String, String)> {
    Ok((
        device
            .id()
            .map_err(|source| Error::DeviceId { source })?
            .to_string(),
        device
            .description()
            .map_err(|source| Error::DeviceName { source })?
            .name()
            .to_owned(),
    ))
}

/// 系统默认输出设备的身份；没有默认设备或读取失败时为 `None`。
pub(super) fn default_id() -> Option<String> {
    cpal::default_host()
        .default_output_device()?
        .id()
        .ok()
        .map(|id| id.to_string())
}

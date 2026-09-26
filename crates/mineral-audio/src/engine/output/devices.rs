//! CPAL 输出设备枚举和按身份解析；不建立输出流。

use color_eyre::eyre::eyre;
use rodio::cpal;
use rodio::cpal::traits::{DeviceTrait, HostTrait};

use crate::{OutputDevice, OutputTarget};

/// 返回 daemon 所在机器的输出设备；单个已拔出的设备不阻止返回其他设备。
pub(super) fn list() -> color_eyre::Result<Vec<OutputDevice>> {
    let host = cpal::default_host();
    let default_id = default_id();
    let mut devices = Vec::new();
    for device in host.output_devices()? {
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
pub(super) fn resolve(target: &OutputTarget) -> color_eyre::Result<cpal::Device> {
    let host = cpal::default_host();
    match target {
        OutputTarget::SystemDefault => host
            .default_output_device()
            .ok_or_else(|| eyre!("no default output device")),
        OutputTarget::Device(id) => {
            let id = id.parse::<cpal::DeviceId>()?;
            host.device_by_id(&id)
                .ok_or_else(|| eyre!("output device is unavailable: {id}"))
        }
    }
}

/// 读取 CPAL 返回的设备 ID 与名称。
pub(super) fn describe(device: &cpal::Device) -> color_eyre::Result<(String, String)> {
    Ok((
        device.id()?.to_string(),
        device.description()?.name().to_owned(),
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

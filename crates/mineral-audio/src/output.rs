//! 输出设备身份、选择目标和已打开输出流的格式，供 daemon 与客户端交换状态。

use serde::{Deserialize, Serialize};

/// daemon 本次运行期间采用的输出选择；设备名只用于展示，身份使用 CPAL ID。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutputTarget {
    /// 跟随系统默认输出设备。
    SystemDefault,

    /// 固定使用给定 CPAL 设备 ID。
    Device(String),
}

/// 一次设备枚举得到的输出设备。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputDevice {
    /// CPAL 的设备身份，作为选择请求的参数。
    pub id: String,

    /// CPAL 返回的设备名称，未经改写。
    pub name: String,

    /// 枚举时是否为系统默认输出设备。
    pub is_default: bool,
}

/// 当前已成功打开并启动的输出流；不是设备默认配置或文件编码格式。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioOutput {
    /// 产生当前输出流的选择目标。
    pub target: OutputTarget,

    /// 实际打开的 CPAL 设备 ID。
    pub device_id: String,

    /// 实际打开的设备名称
    pub device_name: String,

    /// 建流时采用的采样率
    pub sample_rate_hz: u32,

    /// 建流时采用的声道数量
    pub channels: u16,

    /// CPAL 样本格式的名称，例如 `f32`。
    pub sample_format: String,
}

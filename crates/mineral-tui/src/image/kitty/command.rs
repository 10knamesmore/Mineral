//! 构造 Kitty shared memory 传输、virtual placement 与能力查询命令。

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;

use crate::image::graphics::TerminalRelay;

/// 构造 POSIX shared memory 图片传输命令。
///
/// 图片数据只建立 image id，不绑定显示尺寸；virtual placement 在渲染时另行创建。
///
/// # Params:
///   - `image_id`: 非零 Kitty image id
///   - `pixels`: 原始图片像素宽高
///   - `name`: POSIX shared memory 名称
///   - `relay`: 终端 relay 形态
pub(super) fn transmit_shared_memory(
    image_id: u32,
    pixels: (u32, u32),
    name: &str,
    relay: TerminalRelay,
) -> String {
    let payload = STANDARD.encode(name);
    relay.wrap(format!(
        "\x1b_Gq=2,i={image_id},a=t,f=32,t=s,s={},v={};{payload}\x1b\\",
        pixels.0, pixels.1
    ))
}

/// 构造指定 cell 尺寸的 Unicode virtual placement。
///
/// # Params:
///   - `image_id`: 已传输的 Kitty image id
///   - `placement_id`: 由 placement cell 尺寸确定的非零 id
///   - `cells`: placement cell 宽高
///   - `relay`: 终端 relay 形态
pub(super) fn create_virtual_placement(
    image_id: u32,
    placement_id: u32,
    cells: (u16, u16),
    relay: TerminalRelay,
) -> String {
    relay.wrap(format!(
        "\x1b_Gq=2,i={image_id},p={placement_id},a=p,U=1,c={},r={};\x1b\\",
        cells.0, cells.1
    ))
}

/// 构造 POSIX shared memory 能力查询命令。
///
/// # Params:
///   - `image_id`: 查询回执关联 id
///   - `name`: 探针 shared memory 名称
///   - `relay`: 终端 relay 形态
pub(super) fn query_shared_memory(image_id: u32, name: &str, relay: TerminalRelay) -> String {
    let payload = STANDARD.encode(name);
    relay.wrap(format!(
        "\x1b_Gi={image_id},a=q,t=s,f=32,s=1,v=1;{payload}\x1b\\"
    ))
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;

    use super::{create_virtual_placement, query_shared_memory, transmit_shared_memory};
    use crate::image::graphics::TerminalRelay;

    /// shared memory 传输只携带资源名称，placement 独立声明显示尺寸。
    #[test]
    fn shared_memory_transmit_and_placement_are_separate() {
        let name = "/mineral-kitty";
        let encoded = STANDARD.encode(name);
        assert_eq!(
            transmit_shared_memory(11, (16, 32), name, TerminalRelay::Direct),
            format!("\x1b_Gq=2,i=11,a=t,f=32,t=s,s=16,v=32;{encoded}\x1b\\")
        );
        assert_eq!(
            create_virtual_placement(11, 2050, (4, 2), TerminalRelay::Direct),
            "\x1b_Gq=2,i=11,p=2050,a=p,U=1,c=4,r=2;\x1b\\"
        );
        assert_eq!(
            query_shared_memory(12, name, TerminalRelay::Direct),
            format!("\x1b_Gi=12,a=q,t=s,f=32,s=1,v=1;{encoded}\x1b\\")
        );
    }
}

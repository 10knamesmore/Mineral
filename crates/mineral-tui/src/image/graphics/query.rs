//! 查询并解析单个 cell 的像素尺寸。

use std::time::Duration;

use crossterm::terminal::window_size;

use super::protocol::TerminalRelay;
use crate::terminal::exchange;

/// 等待终端响应的上限。
const QUERY_TIMEOUT: Duration = Duration::from_secs(1);

/// 启动时查询字符像素尺寸；终端无响应时沿用窗口查询与估计尺寸。
pub(crate) fn query_cell_pixels(relay: TerminalRelay) -> (u16, u16) {
    cell_pixels(relay)
        .or_else(window_cell_pixels)
        .unwrap_or((10, 20))
}

/// 从当前窗口尺寸计算字符像素宽高，不读取 stdin，可在主循环刷新。
pub(crate) fn window_cell_pixels() -> Option<(u16, u16)> {
    let window = window_size().ok()?;
    if window.columns == 0 || window.rows == 0 || window.width == 0 || window.height == 0 {
        return None;
    }
    Some((window.width / window.columns, window.height / window.rows))
}

/// 向终端查询单 cell 的像素宽高。
///
/// # Params:
///   - `relay`: 终端 relay 形态
///
/// # Return:
///   终端应答的 cell 像素宽高；I/O 失败或终端不应答时返回 `None`
fn cell_pixels(relay: TerminalRelay) -> Option<(u16, u16)> {
    let query = relay.wrap("\x1b[16t\x1b[5n".to_owned());
    match exchange(&query, QUERY_TIMEOUT, has_status_response) {
        Ok(response) => csi_payloads(&response, b't').find_map(parse_cell_pixels),
        Err(error) => {
            mineral_log::debug!(
                target: "tui",
                error = mineral_log::chain(&error),
                "终端 cell 像素查询失败"
            );
            None
        }
    }
}

/// 判断 Device Status Report 是否已经结束响应。
fn has_status_response(response: &[u8]) -> bool {
    csi_payloads(response, b'n').any(|payload| payload == b"0")
}

/// 返回指定 final byte 的 CSI 参数段。
fn csi_payloads(response: &[u8], final_byte: u8) -> impl Iterator<Item = &[u8]> {
    response
        .split(|byte| *byte == 0x1B)
        .filter_map(move |segment| {
            let csi = segment.strip_prefix(b"[")?;
            let end = csi.iter().position(|byte| *byte == final_byte)?;
            csi.get(..end)
        })
}

/// 解析 CSI 16t 对应的 `CSI 6;height;width t` 响应。
fn parse_cell_pixels(payload: &[u8]) -> Option<(u16, u16)> {
    let mut parts = payload.split(|byte| *byte == b';');
    if parts.next()? != b"6" {
        return None;
    }
    let height = parse_u16(parts.next()?)?;
    let width = parse_u16(parts.next()?)?;
    (width > 0 && height > 0).then_some((width, height))
}

/// 解析 ASCII 十进制 `u16`。
fn parse_u16(bytes: &[u8]) -> Option<u16> {
    std::str::from_utf8(bytes).ok()?.parse::<u16>().ok()
}

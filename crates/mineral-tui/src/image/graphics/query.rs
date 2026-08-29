//! 查询并解析 Sixel 与 cell 像素尺寸能力。

use std::time::Duration;

use super::protocol::TerminalRelay;
use super::stdio::exchange;

/// 等待整组终端能力响应的上限。
const QUERY_TIMEOUT: Duration = Duration::from_secs(1);

/// 启动期从终端响应解析出的图形能力。
#[derive(Default)]
pub(super) struct DetectedGraphics {
    /// DA1 是否包含 Sixel 参数 4。
    pub(super) sixel: bool,

    /// CSI 16t 返回的单 cell 像素宽高。
    pub(super) cell_pixels: Option<(u16, u16)>,
}

/// 向终端查询图形能力；I/O 失败时返回空能力并记录诊断。
pub(super) fn detect(relay: TerminalRelay) -> DetectedGraphics {
    let query = relay.wrap("\x1b[c\x1b[16t\x1b[5n".to_owned());
    match exchange(&query, QUERY_TIMEOUT, has_status_response) {
        Ok(response) => parse(&response),
        Err(error) => {
            mineral_log::debug!(
                target: "tui",
                error = mineral_log::chain(&error),
                "终端图形能力查询失败"
            );
            DetectedGraphics::default()
        }
    }
}

/// 解析整组终端响应。
fn parse(response: &[u8]) -> DetectedGraphics {
    DetectedGraphics {
        sixel: csi_payloads(response, b'c').into_iter().any(|payload| {
            payload
                .strip_prefix(b"?")
                .is_some_and(|parameters| parameters.split(|byte| *byte == b';').any(|p| p == b"4"))
        }),
        cell_pixels: csi_payloads(response, b't')
            .into_iter()
            .find_map(parse_cell_pixels),
    }
}

/// 判断 Device Status Report 是否已经结束整组响应。
fn has_status_response(response: &[u8]) -> bool {
    csi_payloads(response, b'n')
        .into_iter()
        .any(|payload| payload == b"0")
}

/// 返回指定 final byte 的 CSI 参数段。
fn csi_payloads(response: &[u8], final_byte: u8) -> Vec<&[u8]> {
    response
        .split(|byte| *byte == 0x1B)
        .filter_map(|segment| {
            let csi = segment.strip_prefix(b"[")?;
            let end = csi.iter().position(|byte| *byte == final_byte)?;
            csi.get(..end)
        })
        .collect::<Vec<&[u8]>>()
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

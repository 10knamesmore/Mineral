//! 启动期探测 Kitty POSIX shared memory 能力。

use std::time::Duration;

use super::command::query_shared_memory;
use super::shared_memory::SharedMemory;
use crate::image::graphics::{TerminalRelay, exchange};

/// POSIX shared memory 探针的回执 id。
const PROBE_ID: u32 = 299;

/// 等待 Kitty 回执的上限。
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// 探测终端是否能读取当前进程创建的 POSIX shared memory object。
///
/// # Params:
///   - `relay`: 终端 relay 形态
///
/// # Return:
///   Kitty shared memory 查询是否返回 OK
pub(crate) fn probe_shared_memory(relay: TerminalRelay) -> bool {
    let result = probe_result(relay);
    if let Err(error) = &result {
        mineral_log::debug!(
            target: "tui",
            error = mineral_log::chain(error),
            "Kitty shared memory 探测失败"
        );
    }
    result.unwrap_or(false)
}

/// 写入 1×1 RGBA 探针并等待对应回执。
fn probe_result(relay: TerminalRelay) -> color_eyre::Result<bool> {
    let resource = SharedMemory::create(PROBE_ID, &[0, 0, 0, 255])?;
    let query = query_shared_memory(PROBE_ID, resource.name(), relay);
    let response = exchange(&query, PROBE_TIMEOUT, |buffer| {
        parse_probe_reply(buffer, PROBE_ID).is_some()
    })?;
    Ok(parse_probe_reply(&response, PROBE_ID).unwrap_or(false))
}

/// 从任意前后缀中解析 `_Gi=<id>;OK` 或错误回执。
fn parse_probe_reply(buffer: &[u8], image_id: u32) -> Option<bool> {
    let marker = format!("Gi={image_id};");
    let position = buffer
        .windows(marker.len())
        .position(|window| window == marker.as_bytes())?;
    let start = position.saturating_add(marker.len());
    let reply = buffer.get(start..)?;
    (reply.len() >= 2).then(|| reply.starts_with(b"OK"))
}

#[cfg(test)]
mod tests {
    use super::parse_probe_reply;

    /// 回执解析按 image id 关联，支持前后有其他终端响应。
    #[test]
    fn parses_probe_reply_for_requested_image() {
        assert_eq!(
            parse_probe_reply(b"noise\x1b_Gi=299;OK\x1b\\tail", 299),
            Some(true)
        );
        assert_eq!(
            parse_probe_reply(b"\x1b_Gi=299;ENOENT:missing\x1b\\", 299),
            Some(false)
        );
        assert_eq!(
            parse_probe_reply(b"\x1b_Gi=300;OK\x1b\\", 299),
            None,
            "其他探针 id 不能误命中"
        );
        assert_eq!(
            parse_probe_reply(b"\x1b_Gi=299;O", 299),
            None,
            "不完整回执须继续等待"
        );
    }
}

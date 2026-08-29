//! 在事件循环启动前执行有截止时间的终端 stdio 往返。

use std::io::{Read as _, Write as _};
use std::os::fd::AsFd as _;
use std::time::{Duration, Instant};

use color_eyre::eyre::WrapErr as _;
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};

/// 写出控制序列并读取响应，直到完成条件命中或截止时间到达。
///
/// 截止时间到达时返回已经收到的字节；I/O 或 poll 失败才返回错误。调用方必须在 TUI
/// 进入 raw mode 后、事件读取开始前调用，避免与键盘事件竞争 stdin。
///
/// # Params:
///   - `command`: 待写 stdout 的完整控制序列
///   - `timeout`: 最长等待时间
///   - `complete`: 判断响应是否完整的函数
///
/// # Return:
///   截止前收到的终端响应
///
/// # Error:
///   stdout 写入、stdin 读取或 poll 失败时返回错误
pub(crate) fn exchange(
    command: &str,
    timeout: Duration,
    complete: impl Fn(&[u8]) -> bool,
) -> color_eyre::Result<Vec<u8>> {
    {
        let mut output = std::io::stdout().lock();
        output
            .write_all(command.as_bytes())
            .wrap_err("write terminal graphics query")?;
        output.flush().wrap_err("flush terminal graphics query")?;
    }
    let deadline = Instant::now() + timeout;
    let input = std::io::stdin();
    let mut input = input.lock();
    let mut response = Vec::<u8>::new();
    loop {
        if complete(&response) {
            return Ok(response);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(response);
        }
        let timeout =
            PollTimeout::try_from(remaining).wrap_err("convert terminal query timeout")?;
        let ready = {
            let mut descriptors = [PollFd::new(input.as_fd(), PollFlags::POLLIN)];
            poll(&mut descriptors, timeout).wrap_err("poll terminal graphics response")?
        };
        if ready == 0 {
            return Ok(response);
        }
        let mut chunk = [0_u8; 1024];
        let read = input
            .read(&mut chunk)
            .wrap_err("read terminal graphics response")?;
        if read == 0 {
            return Ok(response);
        }
        if let Some(bytes) = chunk.get(..read) {
            response.extend_from_slice(bytes);
        }
    }
}

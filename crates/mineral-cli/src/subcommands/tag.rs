//! `mineral tag` — 回填存量落盘文件(下载导出 + 播放缓存)的内嵌 metadata tag。
//!
//! 流程:发 [`Request::TagBackfill`] 拿受理计数 → 轮询 [`Request::TagProgress`]
//! 原地渲染进度条,直到受理任务全部处理完(仿 TUI/CLI 轮询下载进度的既有形态)。

use std::io::Write as _;
use std::time::Duration;

use color_eyre::eyre::bail;
use mineral_protocol::{OneshotClient, Request, Response, TagProgressWire};

/// 进度轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// `mineral tag` 入口:连 daemon socket(含握手)→ 发回填请求 → 轮询渲染进度。
///
/// # Params:
///   - `all`: `--all` 是否给出(批量操作必须显式确认)
pub async fn run(all: bool) -> color_eyre::Result<()> {
    if !all {
        bail!("回填是批量操作,请显式给出 `mineral tag --all`(覆盖全部下载导出 + 播放缓存)");
    }
    let socket_path = mineral_paths::socket_path()?;
    let mut client = OneshotClient::connect(&socket_path).await?;

    let base = progress(&mut client).await?;
    let counts = match client.request(Request::TagBackfill).await? {
        Response::TagBackfill(c) => c,
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    };
    let accepted = u64::from(counts.cached) + u64::from(counts.exported);
    if accepted == 0 {
        println!("没有新受理的打标任务(候选为空 / 全部已在队列 / `download.tagging` 已关闭)");
        return Ok(());
    }
    println!(
        "已受理 {accepted} 个文件(缓存 {} / 导出 {}),开始打标:",
        counts.cached, counts.exported
    );
    // 进度终点 = 当前累计处理数 + 本次受理数;daemon 内单调计数天然换算。
    let target = base.processed + accepted;
    let mut stdout = std::io::stdout();
    let final_failed = loop {
        tokio::time::sleep(POLL_INTERVAL).await;
        let p = progress(&mut client).await?;
        if p.submitted < base.submitted {
            // 计数是 daemon 生命周期内单调累计,回退 = daemon 换过进程。
            println!();
            bail!("daemon 进度计数回退(疑似重启),进度跟踪中断;已受理的打标不受影响");
        }
        let done = p.processed.saturating_sub(base.processed).min(accepted);
        let failed = p.failed.saturating_sub(base.failed);
        print!("\r  {done}/{accepted}(失败 {failed})");
        stdout.flush()?;
        if p.processed >= target {
            break failed;
        }
    };
    println!();
    println!("完成:{accepted} 个已处理,失败 {final_failed} 个(失败详情见 daemon 日志)");
    Ok(())
}

/// 拉一次打标进度快照(配对 [`Request::TagProgress`])。
///
/// # Params:
///   - `client`: 已连接的 oneshot client
///
/// # Return:
///   进度快照(累计计数)。
async fn progress(client: &mut OneshotClient) -> color_eyre::Result<TagProgressWire> {
    match client.request(Request::TagProgress).await? {
        Response::TagProgress(p) => Ok(p),
        Response::Error(msg) => bail!("daemon error: {msg}"),
        other => bail!("unexpected response: {other:?}"),
    }
}

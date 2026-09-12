//! 为后台播放任务让出执行时间，并以限时轮询观察异步状态。

use std::time::Duration;

/// 让出执行若干次,给 fire-and-forget 的 `tokio::spawn(on_played)` 跑完。
pub(super) async fn drain_spawned() {
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
}

/// 轮询断言:在 deadline 内反复检查谓词(hook 拦截是 spawn 的异步任务)。
pub(super) async fn wait_until(mut pred: impl FnMut() -> bool) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        if pred() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

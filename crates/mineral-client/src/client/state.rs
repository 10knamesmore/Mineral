//! 本地播放状态、任务摘要与 PCM 读取，以及播放镜像就绪等待。

use std::sync::Arc;
use std::time::Duration;

use mineral_audio::AudioSnapshot;
use mineral_task::Snapshot;

use super::Client;
use crate::state::PlayerMirror;

impl Client {
    /// 等待播放镜像就绪(首个 Player 快照到达;断连立即返回)。
    ///
    /// # Params:
    ///   - `timeout`: 最长等待
    pub async fn wait_player_ready(&self, timeout: Duration) {
        let mirror = Arc::clone(self.mirror());
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if mirror.read_player(PlayerMirror::queue_ready) || !self.connected() {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// 当前播放锚点(本地镜像;未收到为默认值)。
    #[must_use]
    pub fn playback_snapshot(&self) -> AudioSnapshot {
        self.mirror().read_playback(|playback| *playback.anchor())
    }

    /// 展示用播放位置(本地单调钟推进)。
    #[must_use]
    pub fn playback_position_ms(&self) -> u64 {
        self.mirror()
            .read_playback(|playback| playback.position_ms(std::time::Instant::now()))
    }

    /// 任务摘要(本地镜像);尚未收到为 `None`。
    #[must_use]
    pub fn tasks_snapshot(&self) -> Option<Snapshot> {
        self.mirror().tasks_snapshot()
    }

    /// 取走 PCM 近期窗口样本。
    #[must_use]
    pub fn drain_pcm(&self) -> Vec<f32> {
        self.mirror().drain_pcm()
    }
}

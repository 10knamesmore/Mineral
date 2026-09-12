//! 周期收割任务结果、自动续播、预排与会话保存，并在新连接接入时刷新数据。
//!
//! 后台循环独立于 client 推进播放；新连接先收到缓存快照，再触发各来源刷新。

use std::sync::Arc;
use std::time::Duration;

use super::PlayerCore;
use crate::gapless;

impl PlayerCore {
    /// 长跑后台 loop:每 tick 一次 events drain + auto-next + prefetch 检查。
    pub(super) async fn background_loop(self) {
        let mut tick = tokio::time::interval(Duration::from_millis(self.inner.player_tick_ms));
        loop {
            tick.tick().await;
            self.consume_events_once();
            gapless::check_advance(&self);
            gapless::check_prefetch(&self);
            self.check_props();
            self.check_session_save();
        }
    }

    /// 新 client 的初始数据:先即时下发歌单库缓存快照(有数据才发,消除连接
    /// 瞬间的空库假象),再重拉各源歌单 + 触发收藏同步。收藏走 server 侧 async
    /// 编排(需 persist,不进 task lane),把 canonical favorited 集推给 client。
    pub fn refresh_initial_loads(&self) {
        self.push_cached_library_snapshot();
        // 重连的 client 播放中途接入:补推当前曲的 db 包络(缺失静默,不触发计算)。
        self.replay_current_envelope();
        for ch in &self.inner.channels {
            let source = ch.source();
            self.submit_my_playlists(source);
            let this = self.clone();
            let channel = Arc::clone(ch);
            tokio::spawn(async move {
                this.sync_favorites(source, channel).await;
            });
        }
        // 立即扫一次上一会话遗留的缺 meta 收藏;本次 sync 晚到的那批由各 sync 末尾再触发、
        // pending 合并进同一 worker。
        self.spawn_meta_backfill();
    }
}

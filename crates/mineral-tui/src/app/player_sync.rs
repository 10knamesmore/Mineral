//! 将版本门控的播放镜像投影为队列、在播曲和歌词状态。

use mineral_protocol::PlayerSync;

use super::App;

impl App {
    /// 把 client 镜像的版本门控同步灌进 AppState 投影。每帧调一次。
    ///
    /// 核心语义:**重段缺席 ≠ 清空**——`None` 表示「与已有版本一致」,镜像原地保持;
    /// 只有 `Some` 才整体替换。轻段(play_mode / play_origin)随重段一起到达。
    pub(super) fn apply_player_sync(&mut self, sync: PlayerSync) {
        self.state.player.versions = sync.versions;
        self.state.playback.play_origin = sync.play_origin;
        self.state.playback.mode = sync.play_mode;
        // 在播位置锚点是轻段,每 tick 灌(prev/next 可在 queue 列表不变时单独前进)。
        // 它决定 queue 浮层的在播行样式，独立于客户端的 UI 光标(后者只钳防越界)。
        self.state.player.cursor = sync.cursor;
        if let Some(q) = sync.queue {
            self.state.player.queue = q.queue;
            self.state.player.original_queue = q.original_queue;
            self.overlays.clamp_queue(self.state.player.queue.len());
        }
        // 浮层开着时持续记账:关闭是动画式的(close_top 后浮层还在栈上退场几帧),
        // 挂在关闭那一刻反而要挑时点,不如每 tick 抄一份现值。
        if let Some(at) = self.overlays.active_queue_cursor(&self.state) {
            self.queue_cursor_memo = Some(at);
        }
        if let Some(c) = sync.current {
            // 包络随本段与 current_song 原子到达(归属由 server 组段保证)。同一份包络重复
            // 同曲 media facts 更新时 `sync_envelope` 原地保留,入场动画不重播。
            let song_id = c.current_song.as_ref().map(|s| s.id.clone());
            let ticks = self.state.waveform_reveal_ticks();
            self.state
                .playback
                .sync_envelope(song_id, c.current_envelope, ticks);
            self.state.player.current = c.current_song.clone();
            self.state.playback.track = c.current_song;
            self.state.playback.direct_media = c.direct_media;
            self.state.playback.media_info = c.media_info;
            // lyrics 已在 channel 层结构化清洗,按 current_lyrics_song_id 直接整份收下。
            if let (Some(song_id), Some(lyrics)) = (c.current_lyrics_song_id, c.current_lyrics)
                && !self.state.library.lyrics.contains_key(&song_id)
            {
                self.state.library.lyrics.insert(song_id, lyrics);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_protocol::{PlayerSync, QueueSync};

    use crate::test_support::{app_with_queue, endserenading};

    /// 版本门控的关键语义回归:重段缺席(版本一致的稳态 tick)= 「与已有一致」,
    /// **不是清空** —— queue / current 镜像必须原地保持。
    #[test]
    fn light_only_sync_keeps_queue_and_current() -> color_eyre::Result<()> {
        let mut app = app_with_queue(6, /*current_idx*/ 2)?;
        let queue_before = app.state.player.queue.len();
        let current_before = app.state.player.current.clone();
        assert!(current_before.is_some(), "前置:有在播歌");

        // 稳态 tick:两重段都缺席,只有轻段。
        app.apply_player_sync(PlayerSync::default());

        assert_eq!(
            app.state.player.queue.len(),
            queue_before,
            "queue 不得被清空"
        );
        assert_eq!(
            app.state.player.current, current_before,
            "current 不得被清空"
        );
        Ok(())
    }

    /// 带重段的 sync 正常替换镜像 + 记录版本号供下次回报。
    #[test]
    fn sync_with_sections_replaces_and_records_versions() -> color_eyre::Result<()> {
        let mut app = app_with_queue(2, /*current_idx*/ 0)?;
        let sync = PlayerSync {
            versions: mineral_protocol::PlayerVersions {
                queue: mineral_protocol::SegmentVersion::new(7),
                current: mineral_protocol::SegmentVersion::new(9),
            },
            queue: Some(QueueSync {
                queue: endserenading(4),
                original_queue: None,
            }),
            ..Default::default()
        };
        app.apply_player_sync(sync);
        assert_eq!(app.state.player.queue.len(), 4, "queue 重段应整体替换");
        assert_eq!(
            app.state.player.versions.queue,
            mineral_protocol::SegmentVersion::new(7)
        );
        assert_eq!(
            app.state.player.versions.current,
            mineral_protocol::SegmentVersion::new(9)
        );
        Ok(())
    }
}

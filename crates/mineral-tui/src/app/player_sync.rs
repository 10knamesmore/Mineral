//! 将播放镜像投影为 UI 状态:轻量字段每帧更新,队列与当前曲按版本替换。

use mineral_protocol::PlayerSync;

use super::App;

impl App {
    /// 应用 client 播放镜像的轻量状态,并替换版本有变化的重段。
    ///
    /// 重段为 `None` 表示与已有版本一致,保留原值;为 `Some` 时整体替换。
    /// 模式、光标和来源始终更新,不依赖重段是否变化。
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
        // 同步时记录活跃队列浮层选中项的真实下标,供下次打开时恢复。
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
            self.state.player.current_advance = c.advance;
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
    use mineral_protocol::{PlayMode, PlayerSync, QueueSync};

    use crate::test_support::{app_with_queue, endserenading};

    /// 四档模式都能仅靠轻段更新,重段缺席时保留已有队列与当前曲。
    #[test]
    fn light_only_sync_keeps_queue_and_current() -> color_eyre::Result<()> {
        let mut app = app_with_queue(6, /*current_idx*/ 2)?;
        let queue_before = app.state.player.queue.clone();
        let current_before = app.state.player.current.clone();
        assert!(current_before.is_some(), "前置:有在播歌");

        for mode in [
            PlayMode::Shuffle,
            PlayMode::RepeatAll,
            PlayMode::RepeatOne,
            PlayMode::Sequential,
        ] {
            app.apply_player_sync(PlayerSync {
                play_mode: mode,
                ..Default::default()
            });

            assert_eq!(app.state.playback.mode, mode);
            assert_eq!(app.state.player.queue, queue_before, "queue 应保持原值");
            assert_eq!(
                app.state.player.current, current_before,
                "current 应保持原值"
            );
        }
        Ok(())
    }

    /// 进入档位随 current 段到达并留在镜像里(重段缺席不改它),供切歌转场定向。
    #[test]
    fn current_advance_follows_current_segment() -> color_eyre::Result<()> {
        let mut app = app_with_queue(2, /*current_idx*/ 0)?;
        let song = app
            .state
            .player
            .queue
            .first()
            .cloned()
            .ok_or_else(|| color_eyre::eyre::eyre!("前置:队列应有歌"))?;
        app.apply_player_sync(PlayerSync {
            versions: mineral_protocol::PlayerVersions {
                queue: mineral_protocol::SegmentVersion::ZERO,
                current: mineral_protocol::SegmentVersion::new(3),
            },
            current: Some(mineral_protocol::CurrentSync {
                current_song: Some(song),
                advance: Some(mineral_protocol::AdvanceKind::Prev),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(
            app.state.player.current_advance,
            Some(mineral_protocol::AdvanceKind::Prev)
        );

        // 轻段同步(无 current 重段)不碰它——档位描述的是当前这首歌。
        app.apply_player_sync(PlayerSync::default());
        assert_eq!(
            app.state.player.current_advance,
            Some(mineral_protocol::AdvanceKind::Prev),
            "重段缺席应保留"
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

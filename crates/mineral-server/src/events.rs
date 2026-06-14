//! scheduler 任务事件的消化与分流。
//!
//! 每 tick 一次 drain:`PlayUrlReady` / `LyricsReady` 在 server 内部消化
//! (进 PlayerSync 的 current 重段,不转发);`PlaylistWriteDone` 成功时先触发
//! 缓存收敛再转发;其余原样进 client_events buffer 等 client 拉走。

use mineral_model::{PlayUrl, Song, SongId};
use mineral_task::{ChannelFetchKind, PlaylistWriteOp, Priority, TaskEvent, TaskKind};

use crate::gapless;
use crate::player::PlayerCore;

impl PlayerCore {
    /// 一次 drain scheduler events,分类:PlayUrlReady/LyricsReady 内部消化、
    /// PlaylistWriteDone 成功先收敛缓存、全部业务事件 push 到 client_events buffer。
    pub(crate) fn consume_events_once(&self) {
        let events = self.inner.scheduler.drain_events();
        if events.is_empty() {
            return;
        }
        let mut forward = Vec::with_capacity(events.len());
        for ev in events {
            match ev {
                TaskEvent::PlayUrlReady { song_id, play_url } => {
                    self.handle_play_url_ready(&song_id, play_url);
                }
                TaskEvent::LyricsReady { song_id, lyrics } => {
                    self.handle_lyrics_ready(&song_id, lyrics);
                }
                TaskEvent::PlaylistWriteDone { op, error } => {
                    if error.is_none() {
                        self.refresh_after_write(&op);
                    }
                    forward.push(TaskEvent::PlaylistWriteDone { op, error });
                }
                other => forward.push(other),
            }
        }
        if !forward.is_empty() {
            self.inner.client_events.lock().extend(forward);
        }
    }

    /// 写成功后的缓存收敛:**不直接改数据,只提交重拉任务**——数据重建走
    /// "远端为事实源 + 版本戳"的现有读管线,写路径与读路径永远只有一套真相。
    /// (netease 写后 `trackUpdateTime` 必然变化,自动命中"版本变 → 全拉"分支。)
    fn refresh_after_write(&self, op: &PlaylistWriteOp) {
        match op {
            PlaylistWriteOp::AddSongs { id, .. } | PlaylistWriteOp::RemoveSongs { id, .. } => {
                self.inner.scheduler.submit(
                    TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail { id: id.clone() }),
                    Priority::User,
                );
            }
            PlaylistWriteOp::Create { source, .. } => {
                self.inner.scheduler.submit(
                    TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists { source: *source }),
                    Priority::User,
                );
            }
            PlaylistWriteOp::Delete { id }
            | PlaylistWriteOp::Rename { id, .. }
            | PlaylistWriteOp::SetDescription { id, .. } => {
                self.inner.scheduler.submit(
                    TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists {
                        source: id.namespace(),
                    }),
                    Priority::User,
                );
            }
        }
    }

    /// PlayUrlReady 命中当前歌 → audio.play + 写 play_url;命中正在预拉的下一首 → gapless 预排;否则丢。
    pub(crate) fn handle_play_url_ready(&self, song_id: &SongId, play_url: PlayUrl) {
        // 先在锁内分类(三选一),放锁后再做会重新加锁的动作(play_capturing / gapless 预排)。
        enum Route {
            Current(Option<Box<Song>>),
            Prefetch,
            Drop,
        }
        let route = {
            let mut st = self.inner.state.lock();
            let want = st.current_song.as_ref().map(|t| &t.id);
            if want == Some(song_id) {
                st.play_url = Some(play_url.clone());
                st.bump_current();
                Route::Current(st.current_song.clone().map(Box::new))
            } else if st.prefetch_fired_for.as_ref() == Some(song_id) {
                Route::Prefetch
            } else {
                Route::Drop
            }
        };
        match route {
            Route::Current(song) => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "play", "play url ready");
                if let Some(song) = song {
                    // 拦截桥:无脚本同步直走,有脚本异步裁决(play_url 已在锁内写过,
                    // 桥内回填同值幂等;改写时回填改写值)。
                    crate::hook_bridge::before_play(self, &song, play_url);
                }
            }
            Route::Prefetch => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "prefetch", "play url ready");
                gapless::on_prefetch_url_ready(self, song_id, play_url);
            }
            Route::Drop => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "drop", "play url ready");
            }
        }
    }

    /// LyricsReady 命中当前歌 → 写入 current_lyrics + 配对 song_id;否则丢(只缓存当前歌)。
    pub(crate) fn handle_lyrics_ready(&self, song_id: &SongId, lyrics: mineral_model::Lyrics) {
        let mut st = self.inner.state.lock();
        let want = st.current_song.as_ref().map(|t| &t.id);
        if want == Some(song_id) {
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "store", "lyrics ready");
            st.current_lyrics = Some(lyrics);
            st.current_lyrics_song_id = Some(song_id.clone());
            st.bump_current();
        } else {
            // 非当前歌,无意义,丢(只缓存当前歌)。
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "drop", "lyrics ready");
        }
    }
}

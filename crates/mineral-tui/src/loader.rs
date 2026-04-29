//! 后台拉取 channel 数据 → mpsc 推 UI 事件。错误进 [`crate::applog`]。

use std::sync::Arc;

use mineral_channel_core::MusicChannel;
use mineral_model::{Playlist, PlaylistId, Song};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::applog;

/// 后台任务推给主线程的事件。
#[derive(Debug)]
pub enum LoadEvent {
    /// 一个 channel 的歌单批次。AppState 按到达顺序 append。
    PlaylistsBatch(Vec<Playlist>),

    /// 某个歌单的曲目。失败兜底为空 vec(占位,让 UI 从 loading 切到空列表)。
    PlaylistTracks {
        /// 歌单 id。
        id: PlaylistId,

        /// 曲目。
        tracks: Vec<Song>,
    },
}

/// 对每个 channel 并发拉 `my_playlists`,每条歌单再并发拉 `songs_in_playlist`。
///
/// # Params:
///   - `channels`: 数据源集合(空也合法)
///
/// # Return:
///   主循环用的接收端。
pub fn spawn_initial_load(channels: Vec<Arc<dyn MusicChannel>>) -> UnboundedReceiver<LoadEvent> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<LoadEvent>();
    for channel in channels {
        tokio::spawn(drive_channel(channel, tx.clone()));
    }
    drop(tx);
    rx
}

async fn drive_channel(channel: Arc<dyn MusicChannel>, tx: UnboundedSender<LoadEvent>) {
    let source = channel.source();
    let playlists = match channel.my_playlists().await {
        Ok(v) => v,
        Err(e) => {
            applog::warn(&format!("{source:?}/my_playlists"), &e.to_string());
            return;
        }
    };

    let ids: Vec<PlaylistId> = playlists.iter().map(|p| p.id.clone()).collect();
    if tx.send(LoadEvent::PlaylistsBatch(playlists)).is_err() {
        // 接收端已关闭(UI 退出),静默返回。
        return;
    }

    for id in ids {
        let ch = Arc::clone(&channel);
        let tx_each = tx.clone();
        tokio::spawn(async move {
            match ch.songs_in_playlist(&id).await {
                Ok(tracks) => {
                    let _ = tx_each.send(LoadEvent::PlaylistTracks { id, tracks });
                }
                Err(e) => {
                    applog::warn(
                        &format!("{:?}/songs_in_playlist:{}", ch.source(), id.as_str()),
                        &e.to_string(),
                    );
                    // 发空 vec 占位,UI 从 loading 切到空列表。
                    let _ = tx_each.send(LoadEvent::PlaylistTracks {
                        id,
                        tracks: Vec::new(),
                    });
                }
            }
        });
    }
}

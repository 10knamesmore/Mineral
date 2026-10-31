//! 网易歌单按需加载：浏览逐批检查身份，整张操作分批补齐所有缺失详情。

use async_stream::try_stream;
use futures_util::stream::BoxStream;
use futures_util::{StreamExt, stream};
use mineral_channel_core::store::{CachedPlaylistEntry, NamespaceStore};
use mineral_channel_core::{FetchSnapshot, PlaylistDetail, PlaylistLoad};
use mineral_model::{CachePolicy, PlaylistId, Song, SongId, SourceKind};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::Result;
use crate::config::PlaylistFetchConfig;
use crate::convert::{self, WebPage};
use crate::{api, transport::Transport};
use std::sync::{Arc, Weak};
use tokio::sync::Mutex;

use super::cache;

/// 同一歌单的请求串行复用缓存；不同歌单仍由 scheduler 并行调度。
pub(crate) struct PlaylistLoader {
    /// 构造期注入的请求批次参数。
    config: PlaylistFetchConfig,

    /// 只持有在飞歌单的弱引用，不把历史歌单留成第二份常驻缓存。
    active: Mutex<FxHashMap<PlaylistId, Weak<Mutex<()>>>>,
}

impl PlaylistLoader {
    /// 与 channel 共用生命周期，避免预览和完整加载重复请求同一批歌曲。
    pub(crate) fn new(config: &PlaylistFetchConfig) -> Self {
        Self {
            config: config.clone(),
            active: Mutex::new(FxHashMap::default()),
        }
    }

    /// 先展示已有曲目，再串行检查远端版本并补齐本次意图所需的数据。
    pub(crate) fn load<'a>(
        &'a self,
        transport: &'a Transport,
        persist: Option<&'a dyn NamespaceStore>,
        id: &'a PlaylistId,
        intent: PlaylistLoad,
        cache: CachePolicy,
    ) -> BoxStream<'a, Result<FetchSnapshot<PlaylistDetail>>> {
        Box::pin(try_stream! {
            let cached = match (cache, persist, intent) {
                (CachePolicy::UseCache, Some(store), PlaylistLoad::Preview | PlaylistLoad::Complete) => {
                    cache::try_load_stale(store, id).await
                }
                _ => None,
            };
            if let Some(mut playlist) = cached {
                if intent == PlaylistLoad::Preview {
                    playlist.entries.retain(|entry| {
                        entry.index.get() < self.config.batch_size().get() as u64
                    });
                }
                yield FetchSnapshot::Cached(PlaylistDetail {
                    playlist,
                    complete: false,
                    next_offset: None,
                });
            }
            let lock = {
                let mut active = self.active.lock().await;
                active.retain(|_, entry| entry.strong_count() > 0);
                if let Some(lock) = active.get(id).and_then(Weak::upgrade) {
                    lock
                } else {
                    let lock = Arc::new(Mutex::new(()));
                    active.insert(id.clone(), Arc::downgrade(&lock));
                    lock
                }
            };
            let _serial = lock.lock().await;
            yield FetchSnapshot::Fresh(fetch(transport, persist, &self.config, id, intent, cache).await?);
        })
    }
}

/// 从最新歌单顺序生成结果；完整请求失败时保留已写入的批次，供下次补拉复用。
async fn fetch(
    transport: &Transport,
    persist: Option<&dyn NamespaceStore>,
    config: &PlaylistFetchConfig,
    id: &PlaylistId,
    intent: PlaylistLoad,
    policy: CachePolicy,
) -> Result<PlaylistDetail> {
    let meta = api::playlist::detail(transport, id, 0).await?.playlist;
    let order = meta
        .track_ids
        .iter()
        .map(|track| track.id.to_string())
        .collect::<Vec<_>>();
    let cached = match persist {
        Some(store) if policy == CachePolicy::UseCache => {
            cache::try_rebuild_if_current(store, id, meta.track_update_time, &order)
                .await
                .unwrap_or_default()
        }
        _ => Vec::new(),
    };
    let mut songs = cached
        .into_iter()
        .map(|entry| (entry.song.id.clone(), entry.song))
        .collect::<FxHashMap<_, _>>();
    let ids = order
        .iter()
        .map(|value| SongId::new(SourceKind::NETEASE, value.clone()))
        .collect::<Vec<_>>();
    let checked = match intent {
        PlaylistLoad::Preview => ids.len().min(config.batch_size().get()),
        PlaylistLoad::More { offset } => usize::try_from(offset)?
            .saturating_add(config.batch_size().get())
            .min(ids.len()),
        PlaylistLoad::Complete => ids.len(),
    };
    let missing = missing_songs(&ids[..checked], &songs);
    mineral_log::debug!(target: "netease", playlist = %id, ?intent, total = ids.len(), cached = songs.len(),
        missing = missing.len(), "准备歌单曲目加载");

    // 始终存完整身份顺序，首批晚到不能把完整缓存的成员关系截短。
    let relations = ids
        .iter()
        .enumerate()
        .map(|(index, song_id)| CachedPlaylistEntry {
            index: mineral_model::CollectionIndex::new(index as u64),
            song_id: song_id.clone(),
        })
        .collect::<Vec<_>>();
    let web_url = WebPage::Playlist(meta.id).into_url()?;
    if let Some(scope) = persist
        && let Err(error) = scope
            .put_playlist_cache(
                id,
                Some(&meta.name),
                Some(&web_url),
                Some(meta.track_update_time),
                &relations,
            )
            .await
    {
        if policy == CachePolicy::Reload {
            return Err(error.into());
        }
        mineral_log::warn!(target: "netease", playlist = %id, error = mineral_log::chain(&error), "写入歌单身份顺序失败");
    }
    let batches = missing
        .chunks(config.batch_size().get())
        .map(<[SongId]>::to_vec)
        .collect::<Vec<_>>();
    let mut requests = stream::iter(batches.into_iter().enumerate().map(
        |(batch, ids)| async move {
            let result = api::song::songs_detail(transport, &ids).await;
            (batch, ids.len(), result)
        },
    ))
    .buffer_unordered(config.max_concurrent().get());
    while let Some((batch, requested, result)) = requests.next().await {
        let tracks = match result {
            Ok(tracks) => tracks,
            Err(error) => {
                mineral_log::warn!(target: "netease", playlist = %id, ?intent, batch, requested,
                    error = mineral_log::chain(&error), "歌单歌曲详情批次失败，保留已有缓存");
                return Err(error);
            }
        };
        let fetched = tracks
            .into_iter()
            .map(convert::album_song_to_model)
            .collect::<Result<Vec<_>>>()?;
        mineral_log::debug!(target: "netease", playlist = %id, batch, requested, returned = fetched.len(), "歌单歌曲详情批次完成");
        if fetched.len() != requested {
            mineral_log::warn!(target: "netease", playlist = %id, batch, requested, returned = fetched.len(),
                "部分歌单身份未返回歌曲详情，保留原始位置缺口");
        }
        let refs = fetched.iter().collect::<Vec<_>>();
        if let Some(scope) = persist
            && let Err(error) = scope.upsert_meta_batch(&refs).await
        {
            if policy == CachePolicy::Reload {
                return Err(error.into());
            }
            mineral_log::warn!(target: "netease", playlist = %id, batch, error = mineral_log::chain(&error), "缓存歌单歌曲详情失败");
        }
        songs.extend(fetched.into_iter().map(|song| (song.id.clone(), song)));
    }
    let entries = convert::playlist_entries_from_order(
        &meta.track_ids[..checked],
        songs.into_values().collect(),
    );
    let complete = checked == ids.len();
    mineral_log::info!(target: "netease", playlist = %id, ?intent, total = ids.len(), loaded = entries.len(), complete,
        "歌单曲目加载完成");
    Ok(PlaylistDetail {
        playlist: convert::playlist_info_to_model(&meta, entries)?,
        complete,
        next_offset: (!complete).then_some(checked as u64),
    })
}

/// 相同歌曲在歌单中可重复出现，但缺失详情只请求一次；请求顺序保留首次出现位置。
fn missing_songs(ids: &[SongId], cached: &FxHashMap<SongId, Song>) -> Vec<SongId> {
    let mut seen = FxHashSet::default();
    ids.iter()
        .filter(|id| !cached.contains_key(*id) && seen.insert((*id).clone()))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_details_reuse_cache_without_losing_duplicate_positions() {
        let cached_song = mineral_test::song("1");
        let other = mineral_test::song("2");
        let order = vec![cached_song.id.clone(), other.id.clone(), other.id.clone()];
        let cached = FxHashMap::from_iter([(cached_song.id.clone(), cached_song)]);
        assert_eq!(missing_songs(&order, &cached), vec![other.id]);
    }
}

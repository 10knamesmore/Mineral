//! 按远端歌单顺序重建已缓存的曲目；返回部分数据不代表歌单已经完整。
//!
//! 版本一致时复用 song_meta，缺失详情由歌单加载模块按请求意图补齐。
//! 预览的轻请求失败时可以重建旧关系；完整请求不能用旧缓存冒充成功。

use mineral_model::{CollectionIndex, PlaylistEntry, PlaylistId, SongId, SourceKind};
use mineral_persist::{CachedPlaylistEntry, ServerStore};
use rustc_hash::FxHashMap;

/// 版本比对决策:本地缓存能否直接复用(复用已有 metadata)。纯函数,便于单测。
///
/// "远端为准"体现在:仅当本地有明确版本戳(`Some`)且与远端**完全相等**才复用;
/// 旧缓存无版本戳(`None`)一律不复用,按加载意图重新取数并补上版本戳。
///
/// # Params:
///   - `cached`: 本地缓存的版本戳(旧库 / 未知为 `None`)
///   - `remote`: 远端当前版本戳
///
/// # Return:
///   可复用本地缓存返回 `true`,否则 `false`(需按加载意图取数)。
fn cache_is_current(cached: Option<i64>, remote: i64) -> bool {
    cached == Some(remote)
}

/// 条件刷新命中分支:缓存版本与远端一致时,按【远端 trackIds 顺序】重建
/// `Vec<PlaylistEntry>`。
///
/// 顺序以远端 `track_ids` 为准(它是最新的)。某首 meta 缺失就跳过该首(不致命);
/// 整张一首都重建不出 → 返回 `None`(上层按加载意图取数)。版本不一致 / 无缓存也返回 `None`。
///
/// # Params:
///   - `persist`: 持久化句柄
///   - `id`: 歌单 id
///   - `remote_tut`: 远端版本戳(`trackUpdateTime`)
///   - `remote_track_ids`: 远端全量曲目裸值(保序,最新顺序)
///
/// # Return:
///   命中且能重建出至少一条 relation 返回 `Some(Vec<PlaylistEntry>)`,否则 `None`。
pub async fn try_rebuild_if_current(
    persist: &ServerStore,
    id: &PlaylistId,
    remote_tut: i64,
    remote_track_ids: &[String],
) -> Option<Vec<PlaylistEntry>> {
    let store = persist.scope(SourceKind::NETEASE);
    let entry = match store.get_playlist_cache(id).await {
        Ok(Some(e)) => e,
        Ok(None) => return None,
        Err(e) => {
            mineral_log::warn!(target: "netease", playlist = %id.value(), error = mineral_log::chain(&e), "读歌单缓存失败,回退远端");
            return None;
        }
    };
    if !cache_is_current(entry.track_update_time, remote_tut) {
        mineral_log::debug!(target: "netease", playlist = %id.value(), "歌单版本变更或缓存无版本戳,重新获取所需曲目");
        return None;
    }
    // 命中:按远端最新顺序重建(而非本地缓存顺序),顺序以远端为准。
    // 远端 trackIds 是 API 来的裸字符串,在此边界铸成带 namespace 的 SongId。
    let remote_entries = remote_track_ids
        .iter()
        .zip(0_u64..)
        .map(|(value, index)| CachedPlaylistEntry {
            index: CollectionIndex::new(index),
            song_id: SongId::new(SourceKind::NETEASE, value.clone()),
        })
        .collect::<Vec<CachedPlaylistEntry>>();
    let entries = rebuild(persist, &remote_entries).await;
    if entries.is_empty() {
        return None;
    }
    mineral_log::debug!(target: "netease", playlist = %id.value(), tracks = entries.len(), "歌单缓存命中(版本一致)");
    Some(entries)
}

/// 忽略版本的缓存重建,供远端(含轻请求)失败时降级用(旧数据胜过报错)。
///
/// 按本地缓存自身的曲目顺序重建(此时拿不到远端顺序)。
///
/// # Params:
///   - `persist`: 持久化句柄
///   - `id`: 歌单 id
///
/// # Return:
///   有缓存且能重建出至少一条 relation 返回 `Some(Vec<PlaylistEntry>)`,否则 `None`。
pub async fn try_load_stale(persist: &ServerStore, id: &PlaylistId) -> Option<Vec<PlaylistEntry>> {
    let store = persist.scope(SourceKind::NETEASE);
    let entry = match store.get_playlist_cache(id).await {
        Ok(Some(e)) => e,
        Ok(None) => return None,
        Err(e) => {
            mineral_log::warn!(target: "netease", playlist = %id.value(), error = mineral_log::chain(&e), "读旧歌单缓存失败");
            return None;
        }
    };
    let entries = rebuild(persist, &entry.entries).await;
    if entries.is_empty() {
        None
    } else {
        Some(entries)
    }
}

/// 按来源批量读取 metadata，再按原 relation 顺序重建；缺失只跳过该 index。
///
/// # Params:
///   - `persist`: 持久化句柄
///   - `entries`: explicit index + SongId relation
///
/// # Return:
///   重建出的 relation(保留 index 与顺序);全缺时为空 vec。
async fn rebuild(persist: &ServerStore, entries: &[CachedPlaylistEntry]) -> Vec<PlaylistEntry> {
    let mut by_source = FxHashMap::<SourceKind, Vec<SongId>>::default();
    for entry in entries {
        by_source
            .entry(entry.song_id.namespace())
            .or_default()
            .push(entry.song_id.clone());
    }
    let mut metadata = FxHashMap::default();
    for (source, ids) in by_source {
        match persist.scope(source).get_meta_batch(&ids).await {
            Ok(songs) => metadata.extend(songs),
            Err(error) => {
                mineral_log::warn!(target: "netease", source = source.name(), songs = ids.len(),
                    error = mineral_log::chain(&error), "读取歌单 metadata 批次失败");
            }
        }
    }
    entries
        .iter()
        .filter_map(|entry| {
            metadata.get(&entry.song_id).cloned().map(|song| {
                PlaylistEntry::builder()
                    .index(entry.index)
                    .song(song)
                    .build()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use mineral_model::{CollectionIndex, PlaylistEntry, PlaylistId, SongId, SourceKind};
    use mineral_persist::{CachedPlaylistEntry, ServerStore};

    use super::{cache_is_current, try_load_stale, try_rebuild_if_current};

    /// 写入指定版本及曲目，供缓存重建测试使用。
    async fn seed_cache(
        persist: &ServerStore,
        id: &PlaylistId,
        name: Option<&str>,
        version: Option<i64>,
        entries: &[PlaylistEntry],
    ) -> color_eyre::Result<()> {
        for entry in entries {
            persist
                .scope(entry.song.source())
                .upsert_meta_batch(&[&entry.song])
                .await?;
        }
        let relations = entries
            .iter()
            .map(|entry| CachedPlaylistEntry {
                index: entry.index,
                song_id: entry.song.id.clone(),
            })
            .collect::<Vec<_>>();
        persist
            .scope(SourceKind::NETEASE)
            .put_playlist_cache(id, name, version, &relations)
            .await
    }

    /// 批量 metadata 读取不得合并 relation；相同裸 ID 的不同来源也必须独立。
    #[tokio::test]
    async fn batch_rebuild_preserves_duplicates_and_source_identity() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("test.db")).await?;
        let id = PlaylistId::new(SourceKind::NETEASE, "555");
        let netease = mineral_test::with_name(mineral_test::song("42"), "网易歌曲");
        let bilibili = mineral_test::with_source(
            mineral_test::with_name(mineral_test::song("42"), "视频音轨"),
            SourceKind::BILIBILI,
        );
        let entries = [(4, netease.clone()), (8, bilibili), (12, netease)]
            .into_iter()
            .map(|(index, song)| {
                PlaylistEntry::builder()
                    .index(CollectionIndex::new(index))
                    .song(song)
                    .build()
            })
            .collect::<Vec<_>>();
        seed_cache(&persist, &id, Some("混源歌单"), Some(700), &entries).await?;
        let rebuilt = try_load_stale(&persist, &id)
            .await
            .ok_or_else(|| color_eyre::eyre::eyre!("已写入歌单应能重建"))?;
        assert_eq!(
            rebuilt, entries,
            "各 relation 的坐标、重复次数与歌曲来源都应保持"
        );
        Ok(())
    }

    /// 版本戳完全相等才复用缓存。
    #[test]
    fn current_only_when_equal() {
        assert!(cache_is_current(Some(100), 100));
        assert!(!cache_is_current(Some(99), 100));
        assert!(!cache_is_current(Some(101), 100));
    }

    /// 旧库缓存无版本戳(None)一律不复用,由上层补版本戳。
    #[test]
    fn none_version_never_current() {
        assert!(!cache_is_current(None, 100));
        assert!(!cache_is_current(None, 0));
    }

    /// 版本一致时按【远端 trackIds 顺序】重建,而非本地缓存写入时的顺序。
    #[tokio::test]
    async fn rebuild_uses_remote_order_when_version_matches() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("test.db")).await?;
        let id = PlaylistId::new(SourceKind::NETEASE, "555");

        // 写入时顺序 1,2,3,版本戳 700
        let songs = vec![
            mineral_test::song("10001"),
            mineral_test::song("10002"),
            mineral_test::song("10003"),
        ];
        let entries = PlaylistEntry::enumerate(songs);
        seed_cache(&persist, &id, Some("我的歌单"), Some(700), &entries).await?;

        // 远端版本一致,但 trackIds 给出新顺序 3,1,2 → 重建应跟远端
        let remote_ids = vec!["10003".to_owned(), "10001".to_owned(), "10002".to_owned()];
        let Some(rebuilt) = try_rebuild_if_current(&persist, &id, 700, &remote_ids).await else {
            return Err(color_eyre::eyre::eyre!("版本一致应命中缓存"));
        };
        let got = rebuilt
            .iter()
            .map(|entry| entry.song.id.value().to_owned())
            .collect::<Vec<String>>();
        assert_eq!(got, remote_ids, "应按远端 trackIds 顺序重建");
        Ok(())
    }

    /// 版本不一致时返回 None(上层按加载意图取数)。
    #[tokio::test]
    async fn version_mismatch_misses() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("test.db")).await?;
        let id = PlaylistId::new(SourceKind::NETEASE, "555");
        seed_cache(
            &persist,
            &id,
            Some("我的歌单"),
            Some(700),
            &PlaylistEntry::enumerate(vec![mineral_test::song("10001")]),
        )
        .await?;
        // 远端版本戳变成 800
        let remote_ids = vec!["10001".to_owned()];
        assert!(
            try_rebuild_if_current(&persist, &id, 800, &remote_ids)
                .await
                .is_none(),
            "版本变更不得复用旧歌单快照"
        );
        Ok(())
    }

    /// 完全无缓存时 try_rebuild_if_current 返回 None。
    #[tokio::test]
    async fn rebuild_miss_returns_none() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("test.db")).await?;
        let id = PlaylistId::new(SourceKind::NETEASE, "999");
        assert!(
            try_rebuild_if_current(&persist, &id, 1, &["x".to_owned()])
                .await
                .is_none(),
            "无缓存应 miss"
        );
        Ok(())
    }

    /// 缓存写回后 try_load_stale 能按缓存自身顺序重建(降级路径)。
    #[tokio::test]
    async fn stale_rebuilds_in_cached_order() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("test.db")).await?;
        let id = PlaylistId::new(SourceKind::NETEASE, "555");
        let songs = vec![mineral_test::song("10001"), mineral_test::song("10002")];
        let entries = PlaylistEntry::enumerate(songs);
        seed_cache(&persist, &id, Some("我的歌单"), Some(700), &entries).await?;

        let Some(rebuilt) = try_load_stale(&persist, &id).await else {
            return Err(color_eyre::eyre::eyre!("有缓存应能降级重建"));
        };
        let got = rebuilt
            .iter()
            .map(|entry| entry.song.id.value().to_owned())
            .collect::<Vec<String>>();
        assert_eq!(got, vec!["10001", "10002"], "降级按缓存顺序重建");
        Ok(())
    }

    /// Missing metadata 只留下 index gap，不把后续 authoritative index 压紧。
    #[tokio::test]
    async fn rebuild_preserves_gap_when_metadata_is_missing() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("test.db")).await?;
        let id = PlaylistId::new(SourceKind::NETEASE, "555");
        let scope = persist.scope(SourceKind::NETEASE);
        scope.upsert_meta(&mineral_test::song("10001")).await?;
        scope.upsert_meta(&mineral_test::song("10003")).await?;
        scope
            .put_playlist_cache(
                &id,
                Some("我的歌单"),
                Some(700),
                &[
                    CachedPlaylistEntry {
                        index: CollectionIndex::new(0),
                        song_id: SongId::new(SourceKind::NETEASE, "10001"),
                    },
                    CachedPlaylistEntry {
                        index: CollectionIndex::new(1),
                        song_id: SongId::new(SourceKind::NETEASE, "10002"),
                    },
                    CachedPlaylistEntry {
                        index: CollectionIndex::new(2),
                        song_id: SongId::new(SourceKind::NETEASE, "10003"),
                    },
                ],
            )
            .await?;
        let Some(rebuilt) = try_load_stale(&persist, &id).await else {
            return Err(color_eyre::eyre::eyre!("有剩余 metadata 应能重建"));
        };
        assert_eq!(
            rebuilt
                .iter()
                .map(|entry| entry.index.get())
                .collect::<Vec<u64>>(),
            vec![0, 2]
        );
        Ok(())
    }
}

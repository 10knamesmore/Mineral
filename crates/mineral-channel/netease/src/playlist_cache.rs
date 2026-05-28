//! 歌单曲目的本地缓存接线(版本号条件刷新)。
//!
//! `songs_in_playlist` 是大头(一个歌单上千首),给它配一层 persist 缓存。
//! 刷新策略是**版本号(`trackUpdateTime`)条件刷新,远端为准**:
//! - 先轻量拿远端版本戳 + 全量 trackIds 顺序(不拉完整 tracks)。
//! - 缓存命中且版本戳一致 → 用本地 song_meta 按【远端 trackIds 顺序】重建 `Vec<Song>`
//!   (顺序以远端为准),省掉拉上千首完整 tracks 的开销。
//! - 版本变 / 无缓存 / 旧缓存无版本戳 → 全拉远端覆盖,写回(含新版本戳)。
//! - 轻请求网络失败 → 降级旧缓存(忽略版本),体验优先;无缓存才冒泡 Err。
//!
//! **缓存只是优化、永不是正确性依赖**:远端是事实来源,版本戳一变即全拉覆盖,
//! 命中也以远端 trackIds 顺序重建。重建时 meta 缺失的歌跳过——宁可缺几首也不返回
//! 脏数据;整张全缺则视作未命中,触发全拉。

use mineral_model::{PlaylistId, Song, SongId, SourceKind};
use mineral_persist::ServerStore;

/// 版本比对决策:本地缓存能否直接复用(免全拉)。纯函数,便于单测。
///
/// "远端为准"体现在:仅当本地有明确版本戳(`Some`)且与远端**完全相等**才复用;
/// 旧缓存无版本戳(`None`)一律不复用,走全拉补上版本戳。
///
/// # Params:
///   - `cached`: 本地缓存的版本戳(旧库 / 未知为 `None`)
///   - `remote`: 远端当前版本戳
///
/// # Return:
///   可复用本地缓存返回 `true`,否则 `false`(需全拉)。
fn cache_is_current(cached: Option<i64>, remote: i64) -> bool {
    cached == Some(remote)
}

/// 条件刷新命中分支:缓存版本与远端一致时,按【远端 trackIds 顺序】重建 `Vec<Song>`。
///
/// 顺序以远端 `track_ids` 为准(它是最新的)。某首 meta 缺失就跳过该首(不致命);
/// 整张一首都重建不出 → 返回 `None`(触发上层全拉)。版本不一致 / 无缓存也返回 `None`。
///
/// # Params:
///   - `persist`: 持久化句柄
///   - `id`: 歌单 id
///   - `remote_tut`: 远端版本戳(`trackUpdateTime`)
///   - `remote_track_ids`: 远端全量曲目裸值(保序,最新顺序)
///
/// # Return:
///   命中且能重建出至少一首返回 `Some(Vec<Song>)`,否则 `None`。
pub async fn try_rebuild_if_current(
    persist: &ServerStore,
    id: &PlaylistId,
    remote_tut: i64,
    remote_track_ids: &[String],
) -> Option<Vec<Song>> {
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
        mineral_log::debug!(target: "netease", playlist = %id.value(), "歌单版本变更或缓存无版本戳,回退全拉");
        return None;
    }
    // 命中:按远端最新顺序重建(而非本地缓存顺序),顺序以远端为准。
    // 远端 trackIds 是 API 来的裸字符串,在此边界铸成带 namespace 的 SongId。
    let remote_ids = remote_track_ids
        .iter()
        .map(|v| SongId::new(SourceKind::NETEASE, v.clone()))
        .collect::<Vec<SongId>>();
    let songs = rebuild(persist, &remote_ids).await;
    if songs.is_empty() {
        return None;
    }
    mineral_log::debug!(target: "netease", playlist = %id.value(), tracks = songs.len(), "歌单缓存命中(版本一致)");
    Some(songs)
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
///   有缓存且能重建出至少一首返回 `Some(Vec<Song>)`,否则 `None`。
pub async fn try_load_stale(persist: &ServerStore, id: &PlaylistId) -> Option<Vec<Song>> {
    let store = persist.scope(SourceKind::NETEASE);
    let entry = match store.get_playlist_cache(id).await {
        Ok(Some(e)) => e,
        Ok(None) => return None,
        Err(e) => {
            mineral_log::warn!(target: "netease", playlist = %id.value(), error = mineral_log::chain(&e), "读旧歌单缓存失败");
            return None;
        }
    };
    let songs = rebuild(persist, &entry.track_values).await;
    if songs.is_empty() { None } else { Some(songs) }
}

/// 把曲目 id 逐个 `get_meta` 重建成 `Vec<Song>`(保序),meta 缺失的跳过。
///
/// # Params:
///   - `persist`: 持久化句柄
///   - `track_ids`: 曲目 id(按给定顺序)
///
/// # Return:
///   重建出的歌曲(保序);全缺时为空 vec。
async fn rebuild(persist: &ServerStore, track_ids: &[SongId]) -> Vec<Song> {
    let store = persist.scope(SourceKind::NETEASE);
    let mut out = Vec::with_capacity(track_ids.len());
    for sid in track_ids {
        match store.get_meta(sid).await {
            Ok(Some(song)) => out.push(song),
            Ok(None) => {}
            Err(e) => {
                mineral_log::warn!(target: "netease", song = %sid.value(), error = mineral_log::chain(&e), "读 song_meta 失败,跳过该首");
            }
        }
    }
    out
}

/// 远端全拉到歌单后写回缓存:每首 `upsert_meta` + 整张 `put_playlist_cache`(含版本戳)。
///
/// best-effort:任一步失败只 warn,不影响返回给上层的远端结果。
///
/// # Params:
///   - `persist`: 持久化句柄
///   - `id`: 歌单 id
///   - `name`: 歌单名(可空)
///   - `track_update_time`: 远端版本戳(`trackUpdateTime`,可空)
///   - `songs`: 远端拉到的歌曲
pub async fn store(
    persist: &ServerStore,
    id: &PlaylistId,
    name: Option<&str>,
    track_update_time: Option<i64>,
    songs: &[Song],
) {
    let scope = persist.scope(SourceKind::NETEASE);
    let mut track_values = Vec::with_capacity(songs.len());
    for song in songs {
        if let Err(e) = scope.upsert_meta(song).await {
            mineral_log::warn!(target: "netease", song = %song.id.value(), error = mineral_log::chain(&e), "upsert song_meta 失败");
        }
        track_values.push(song.id.clone());
    }
    if let Err(e) = scope
        .put_playlist_cache(id, name, track_update_time, &track_values)
        .await
    {
        mineral_log::warn!(target: "netease", playlist = %id.value(), error = mineral_log::chain(&e), "写歌单缓存失败");
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::{PlaylistId, SourceKind};
    use mineral_persist::ServerStore;

    use super::{cache_is_current, store, try_load_stale, try_rebuild_if_current};

    /// 版本戳完全相等才复用缓存。
    #[test]
    fn current_only_when_equal() {
        assert!(cache_is_current(Some(100), 100));
        assert!(!cache_is_current(Some(99), 100));
        assert!(!cache_is_current(Some(101), 100));
    }

    /// 旧库缓存无版本戳(None)一律不复用,走全拉补版本戳。
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
        store(&persist, &id, Some("我的歌单"), Some(700), &songs).await;

        // 远端版本一致,但 trackIds 给出新顺序 3,1,2 → 重建应跟远端
        let remote_ids = vec!["10003".to_owned(), "10001".to_owned(), "10002".to_owned()];
        let Some(rebuilt) = try_rebuild_if_current(&persist, &id, 700, &remote_ids).await else {
            return Err(color_eyre::eyre::eyre!("版本一致应命中缓存"));
        };
        let got = rebuilt
            .iter()
            .map(|s| s.id.value().to_owned())
            .collect::<Vec<String>>();
        assert_eq!(got, remote_ids, "应按远端 trackIds 顺序重建");
        Ok(())
    }

    /// 版本不一致时返回 None(触发上层全拉)。
    #[tokio::test]
    async fn version_mismatch_misses() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("test.db")).await?;
        let id = PlaylistId::new(SourceKind::NETEASE, "555");
        store(
            &persist,
            &id,
            Some("我的歌单"),
            Some(700),
            &[mineral_test::song("10001")],
        )
        .await;
        // 远端版本戳变成 800
        let remote_ids = vec!["10001".to_owned()];
        assert!(
            try_rebuild_if_current(&persist, &id, 800, &remote_ids)
                .await
                .is_none(),
            "版本变更应 miss → 全拉"
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

    /// store 写回后 try_load_stale 能按缓存自身顺序重建(降级路径)。
    #[tokio::test]
    async fn stale_rebuilds_in_cached_order() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let persist = ServerStore::open(&dir.path().join("test.db")).await?;
        let id = PlaylistId::new(SourceKind::NETEASE, "555");
        let songs = vec![mineral_test::song("10001"), mineral_test::song("10002")];
        store(&persist, &id, Some("我的歌单"), Some(700), &songs).await;

        let Some(rebuilt) = try_load_stale(&persist, &id).await else {
            return Err(color_eyre::eyre::eyre!("有缓存应能降级重建"));
        };
        let got = rebuilt
            .iter()
            .map(|s| s.id.value().to_owned())
            .collect::<Vec<String>>();
        assert_eq!(got, vec!["10001", "10002"], "降级按缓存顺序重建");
        Ok(())
    }
}

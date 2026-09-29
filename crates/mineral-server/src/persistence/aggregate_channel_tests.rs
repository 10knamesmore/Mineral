//! Verifies the built-in channel through the daemon’s real library store.

use crate::persistence::ServerStore;
use mineral_channel_core::{Error, MusicChannel};
use mineral_model::{SongId, SourceKind};
use mineral_test::{song, with_name};

use mineral_channel_mineral::{MineralChannel, favorites_playlist_id};
use std::sync::Arc;

/// 造一个含两源收藏的 store:netease「Palisade」+ bilibili「夜間飛行」,
/// 外加一条 loved 但无 meta 的幽灵行(应被跳过)。TempDir 须由调用方持有到测试尾。
async fn store_with_favorites() -> color_eyre::Result<(tempfile::TempDir, ServerStore)> {
    let dir = tempfile::tempdir()?;
    let store = ServerStore::open(&dir.path().join("t.db")).await?;
    let netease = store.scope(SourceKind::NETEASE);
    let n1 = with_name(song("n1"), "Palisade");
    netease.upsert_meta(&n1).await?;
    netease.set_loved(&n1.id, true).await?;
    let bilibili = store.scope(SourceKind::BILIBILI);
    let mut b1 = with_name(song("b1"), "夜間飛行");
    b1.id = SongId::new(SourceKind::BILIBILI, "b1");
    bilibili.upsert_meta(&b1).await?;
    bilibili.set_loved(&b1.id, true).await?;
    netease
        .set_loved(&SongId::new(SourceKind::NETEASE, "ghost"), true)
        .await?;
    Ok((dir, store))
}

/// my_playlists:恰好一张 synthetic 歌单,id/name 固定,track_count 只计
/// join 到 meta 的收藏(幽灵行不计),songs 空(列表面不带载荷)。
#[tokio::test]
async fn my_playlists_is_single_synthetic() -> color_eyre::Result<()> {
    let (_dir, store) = store_with_favorites().await?;
    let ch = MineralChannel::new(Arc::new(store));
    let lists = ch.my_playlists().await?;
    assert_eq!(lists.len(), 1);
    let p = lists
        .first()
        .ok_or_else(|| color_eyre::eyre::eyre!("应有一张歌单"))?;
    assert_eq!(p.id, favorites_playlist_id());
    assert_eq!(p.name, "Favorites");
    assert_eq!(p.track_count, 2, "幽灵行(无 meta)不计入");
    assert!(p.entries.is_empty(), "列表面不带曲目载荷");
    Ok(())
}

/// playlist_detail:favorites id 出全曲目(entered_at DESC,同批按 namespace/value 稳定),
/// relation 从 0 连续编号且源 namespace 保留;其他 id 一律 NotSupported。
#[tokio::test]
async fn playlist_detail_aggregates_and_rejects_unknown() -> color_eyre::Result<()> {
    let (_dir, store) = store_with_favorites().await?;
    let ch = MineralChannel::new(Arc::new(store));
    let p = ch
        .playlist_detail(
            &favorites_playlist_id(),
            mineral_channel_core::PlaylistLoad::Complete,
        )
        .await?
        .playlist;
    assert_eq!(p.track_count, 2);
    assert_eq!(p.entries.len(), 2, "detail 带全曲目,与 track_count 同口径");
    let names = p
        .entries
        .iter()
        .map(|entry| entry.song.name.as_str())
        .collect::<Vec<_>>();
    let indexes = p
        .entries
        .iter()
        .map(|entry| entry.index.get())
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec!["夜間飛行", "Palisade"],
        "后收藏的 bilibili 曲目排在先收藏的 netease 曲目前"
    );
    assert_eq!(indexes, vec![0, 1]);
    assert_eq!(
        p.entries.first().map(|entry| entry.song.source()),
        Some(SourceKind::BILIBILI)
    );
    assert_eq!(
        p.entries.get(1).map(|entry| entry.song.source()),
        Some(SourceKind::NETEASE)
    );

    let other = mineral_model::PlaylistId::new(SourceKind::MINERAL, "nope");
    assert!(
        matches!(
            ch.playlist_detail(&other, mineral_channel_core::PlaylistLoad::Complete)
                .await,
            Err(Error::NotSupported)
        ),
        "未知 id 不臆造歌单"
    );
    Ok(())
}

/// 降级 store(无 pool):歌单仍在,只是空——聚合视图不因 persist 降级而消失。
#[tokio::test]
async fn disabled_store_yields_empty_favorites() -> color_eyre::Result<()> {
    let ch = MineralChannel::new(Arc::new(ServerStore::disabled()));
    let p = ch
        .playlist_detail(
            &favorites_playlist_id(),
            mineral_channel_core::PlaylistLoad::Complete,
        )
        .await?
        .playlist;
    assert_eq!(p.track_count, 0);
    assert!(p.entries.is_empty());
    Ok(())
}

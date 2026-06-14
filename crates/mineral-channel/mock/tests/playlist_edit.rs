//! `MockChannel` 歌单写操作的内存实现契约:
//! 建/删/加歌/删歌/改名/改描述全回路,以及模拟网易云"歌曲已存在"的
//! code 502 语义(供 TUI"已在歌单中" toast 的集成测试使用)。
//!
//! 本文件需要 `--features mock` 才会编译运行(`cargo t` 默认不含,CI 单独兜)。
#![cfg(feature = "mock")]

use color_eyre::eyre::eyre;
use mineral_channel_core::{Error, MusicChannel};
use mineral_channel_mock::MockChannel;
use mineral_model::{Song, SourceKind};

/// 从 demo 数据里借一首已有歌(加歌测试的素材)。
async fn donor_song(chan: &MockChannel) -> color_eyre::Result<Song> {
    let playlists = chan.my_playlists().await?;
    let song = playlists
        .iter()
        .flat_map(|p| p.songs.iter())
        .next()
        .ok_or_else(|| eyre!("demo 数据为空"))?;
    Ok(song.clone())
}

#[tokio::test]
async fn create_playlist_is_listed_with_mock_namespace() -> color_eyre::Result<()> {
    let chan = MockChannel::new();
    let before = chan.my_playlists().await?.len();

    let created = chan.create_playlist("新歌单").await?;
    assert_eq!(created.id.namespace(), SourceKind::MOCK);
    assert_eq!(created.name, "新歌单");
    assert!(created.songs.is_empty());

    let after = chan.my_playlists().await?;
    assert_eq!(after.len(), before + 1);
    assert!(after.iter().any(|p| p.id == created.id));
    Ok(())
}

#[tokio::test]
async fn created_playlists_get_distinct_ids_even_with_same_name() -> color_eyre::Result<()> {
    let chan = MockChannel::new();
    let a = chan.create_playlist("同名").await?;
    let b = chan.create_playlist("同名").await?;
    assert_ne!(a.id, b.id);
    Ok(())
}

#[tokio::test]
async fn add_songs_appends_and_duplicate_maps_to_api_502() -> color_eyre::Result<()> {
    let chan = MockChannel::new();
    let pl = chan.create_playlist("收藏夹").await?;
    let song = donor_song(&chan).await?;

    chan.playlist_add_songs(&pl.id, std::slice::from_ref(&song.id))
        .await?;
    let songs = chan.playlist_detail(&pl.id).await?.songs;
    assert_eq!(
        songs.iter().map(|s| &s.id).collect::<Vec<_>>(),
        vec![&song.id]
    );

    let dup = chan
        .playlist_add_songs(&pl.id, std::slice::from_ref(&song.id))
        .await;
    assert!(matches!(dup, Err(Error::Api { code: 502, .. })));
    // 失败的重复添加不改变内容
    assert_eq!(chan.playlist_detail(&pl.id).await?.songs.len(), 1);
    Ok(())
}

#[tokio::test]
async fn remove_songs_removes_and_ignores_missing() -> color_eyre::Result<()> {
    let chan = MockChannel::new();
    let pl = chan.create_playlist("临时").await?;
    let song = donor_song(&chan).await?;
    chan.playlist_add_songs(&pl.id, std::slice::from_ref(&song.id))
        .await?;

    chan.playlist_remove_songs(&pl.id, std::slice::from_ref(&song.id))
        .await?;
    assert!(chan.playlist_detail(&pl.id).await?.songs.is_empty());

    // 再删同一首(已不存在):宽容忽略,不报错
    chan.playlist_remove_songs(&pl.id, std::slice::from_ref(&song.id))
        .await?;
    Ok(())
}

#[tokio::test]
async fn rename_and_set_description_are_reflected() -> color_eyre::Result<()> {
    let chan = MockChannel::new();
    let pl = chan.create_playlist("旧名").await?;

    chan.rename_playlist(&pl.id, "新名").await?;
    chan.set_playlist_description(&pl.id, "新描述").await?;

    let listed = chan.my_playlists().await?;
    let found = listed
        .iter()
        .find(|p| p.id == pl.id)
        .ok_or_else(|| eyre!("改名后歌单消失"))?;
    assert_eq!(found.name, "新名");
    assert_eq!(found.description, "新描述");
    Ok(())
}

#[tokio::test]
async fn delete_playlist_removes_and_unknown_id_is_api_error() -> color_eyre::Result<()> {
    let chan = MockChannel::new();
    let pl = chan.create_playlist("将删除").await?;

    chan.delete_playlist(&pl.id).await?;
    assert!(!chan.my_playlists().await?.iter().any(|p| p.id == pl.id));

    // 对不存在的歌单操作 → Api 错误(非 panic、非静默成功)
    assert!(matches!(
        chan.delete_playlist(&pl.id).await,
        Err(Error::Api { .. })
    ));
    assert!(matches!(
        chan.rename_playlist(&pl.id, "x").await,
        Err(Error::Api { .. })
    ));
    Ok(())
}

#[tokio::test]
async fn caps_declares_playlist_edit() {
    let chan = MockChannel::new();
    assert!(*chan.caps().playlist_edit());
}

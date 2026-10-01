//! Verifies channel-visible namespace isolation against real SQLite stores.

use mineral_channel_core::store::{NamespaceStore, StoreError, StoreResult};
use mineral_model::{AlbumId, ArtistId, Envelope, PlaylistId, SourceKind, StoreValue};
use mineral_test::{song, with_name, with_source};

use super::ServerStore;

/// Requires a structured source mismatch through the channel interface.
fn assert_foreign<T>(result: &StoreResult<T>) {
    assert!(matches!(
        result,
        Err(StoreError::NamespaceMismatch {
            expected,
            actual,
        }) if *expected == SourceKind::NETEASE && *actual == SourceKind::LOCAL
    ));
}

/// A foreign ID cannot be read or reinterpreted as a same-valued local identity.
#[tokio::test]
async fn channel_reads_reject_foreign_identities() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = ServerStore::open(&dir.path().join("store.db")).await?;
    let netease = store.scope(SourceKind::NETEASE);
    let local = store.scope(SourceKind::LOCAL);
    let own = with_name(song("42"), "netease");
    let foreign = with_source(with_name(song("42"), "local"), SourceKind::LOCAL);
    netease.upsert_meta(&own).await?;
    local.upsert_meta(&foreign).await?;
    let channel: &dyn NamespaceStore = &netease;
    assert_eq!(channel.get_meta(&own.id).await?, Some(own.clone()));
    assert_eq!(local.get_meta(&foreign.id).await?, Some(foreign.clone()));
    assert_foreign(&channel.get_meta(&foreign.id).await);
    assert_foreign(
        &channel
            .get_meta_batch(&[own.id.clone(), foreign.id.clone()])
            .await,
    );
    assert_foreign(
        &channel
            .album_name(&AlbumId::new(SourceKind::LOCAL, "42"))
            .await,
    );
    assert_foreign(
        &channel
            .artist_name(&ArtistId::new(SourceKind::LOCAL, "42"))
            .await,
    );
    assert_foreign(
        &channel
            .get_playlist_cache(&PlaylistId::new(SourceKind::LOCAL, "42"))
            .await,
    );
    assert_foreign(&channel.is_loved(&foreign.id).await);
    assert_foreign(&channel.kv_get(&foreign.id, "key").await);
    assert_foreign(&channel.query_rating(&foreign.id).await);
    assert_foreign(&channel.get_envelope(&foreign.id, 1).await);
    Ok(())
}

/// Foreign writes fail before changing either source, including disabled-store calls.
#[tokio::test]
async fn channel_writes_reject_foreign_identities() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = ServerStore::open(&dir.path().join("store.db")).await?;
    let foreign = with_source(song("42"), SourceKind::LOCAL);
    for store in [store.clone(), ServerStore::disabled()] {
        let scope = store.scope(SourceKind::NETEASE);
        let channel: &dyn NamespaceStore = &scope;
        assert_foreign(&channel.upsert_meta(&foreign).await);
        assert_foreign(&channel.set_loved(&foreign.id, true).await);
        assert_foreign(
            &channel
                .kv_set(&foreign.id, "key", &StoreValue::Int(7))
                .await,
        );
        assert_foreign(&channel.kv_inc(&foreign.id, "key", 1).await);
        assert_foreign(&channel.set_rating(&foreign.id, Some(5)).await);
        assert_foreign(
            &channel
                .put_envelope(
                    &foreign.id,
                    &Envelope {
                        points: vec![1],
                        version: 1,
                    },
                )
                .await,
        );
        assert_foreign(
            &channel
                .put_playlist_cache(&PlaylistId::new(SourceKind::LOCAL, "42"), None, None, &[])
                .await,
        );
        assert!(channel.list_meta().await?.is_empty());
        assert!(channel.loved_ids().await?.is_empty());
    }
    assert!(store.scope(SourceKind::LOCAL).list_meta().await?.is_empty());
    Ok(())
}

/// An invalid tail must neither partially merge a batch nor clear the old source projection.
#[tokio::test]
async fn mixed_source_batches_leave_existing_metadata_unchanged() -> color_eyre::Result<()> {
    let dir = tempfile::tempdir()?;
    let store = ServerStore::open(&dir.path().join("store.db")).await?;
    let scope = store.scope(SourceKind::NETEASE);
    let channel: &dyn NamespaceStore = &scope;
    let original = with_name(song("42"), "original");
    channel.upsert_meta(&original).await?;
    let replacement = with_name(song("42"), "replacement");
    let foreign = with_source(song("tail"), SourceKind::LOCAL);
    assert_foreign(&channel.upsert_meta_batch(&[&replacement, &foreign]).await);
    assert_foreign(&channel.replace_meta_batch(&[&replacement, &foreign]).await);
    assert_eq!(channel.list_meta().await?, vec![original]);
    Ok(())
}

//! `MusicChannel` 新增方法默认实现的契约测试:
//! 写操作与 `artist_albums` 未被实现方覆盖时,必须返回 [`Error::NotSupported`],
//! 保证老 channel 在 trait 扩展后行为不变(运行时兜底,与 caps 声明互为防线)。

use async_trait::async_trait;
use mineral_channel_core::{ChannelCaps, Credential, Error, MusicChannel, Page, SearchHits};
use mineral_model::{
    Album, AlbumId, ArtistId, BitRate, Lyrics, PlayUrl, Playlist, PlaylistId, SearchKind, Song,
    SongId, SourceKind,
};

/// 只实现必需方法的最小桩 channel,所有可选能力全部走 trait 默认实现。
struct BareChannel;

#[async_trait]
impl MusicChannel for BareChannel {
    fn source(&self) -> SourceKind {
        SourceKind::LOCAL
    }

    fn caps(&self) -> ChannelCaps {
        ChannelCaps::builder()
            .searchable(Vec::new())
            .playlist_edit(false)
            .artist_sections(mineral_channel_core::ArtistSections::new(vec![
                mineral_channel_core::ArtistSectionKind::TopSongs,
                mineral_channel_core::ArtistSectionKind::Albums,
            ]))
            .build()
    }

    async fn search_songs(
        &self,
        _query: &str,
        _page: Page,
    ) -> mineral_channel_core::Result<SearchHits<Song>> {
        Err(Error::NotSupported)
    }

    async fn search_albums(
        &self,
        _query: &str,
        _page: Page,
    ) -> mineral_channel_core::Result<SearchHits<Album>> {
        Err(Error::NotSupported)
    }

    async fn search_playlists(
        &self,
        _query: &str,
        _page: Page,
    ) -> mineral_channel_core::Result<SearchHits<Playlist>> {
        Err(Error::NotSupported)
    }

    async fn songs_detail(&self, _ids: &[SongId]) -> mineral_channel_core::Result<Vec<Song>> {
        Err(Error::NotSupported)
    }

    async fn album_detail(&self, _id: &AlbumId) -> mineral_channel_core::Result<Album> {
        Err(Error::NotSupported)
    }

    async fn playlist_detail(&self, _id: &PlaylistId) -> mineral_channel_core::Result<Playlist> {
        Err(Error::NotSupported)
    }

    async fn song_urls(
        &self,
        _ids: &[SongId],
        _quality: BitRate,
    ) -> mineral_channel_core::Result<Vec<PlayUrl>> {
        Err(Error::NotSupported)
    }

    async fn lyrics(&self, _id: &SongId) -> mineral_channel_core::Result<Lyrics> {
        Err(Error::NotSupported)
    }
}

/// 避免未使用警告:桩 channel 不需要登录,但 Credential 导入用于确认 trait 面没变。
#[allow(dead_code)]
fn credential_type_still_exported(c: Credential) -> Credential {
    c
}

#[tokio::test]
async fn playlist_writes_default_to_not_supported() -> color_eyre::Result<()> {
    let chan = BareChannel;
    let pl = PlaylistId::new(SourceKind::LOCAL, "pl-1");
    let song = SongId::new(SourceKind::LOCAL, "song-1");

    assert!(matches!(
        chan.create_playlist("新歌单").await,
        Err(Error::NotSupported)
    ));
    assert!(matches!(
        chan.delete_playlist(&pl).await,
        Err(Error::NotSupported)
    ));
    assert!(matches!(
        chan.playlist_add_songs(&pl, std::slice::from_ref(&song))
            .await,
        Err(Error::NotSupported)
    ));
    assert!(matches!(
        chan.playlist_remove_songs(&pl, std::slice::from_ref(&song))
            .await,
        Err(Error::NotSupported)
    ));
    assert!(matches!(
        chan.rename_playlist(&pl, "改名").await,
        Err(Error::NotSupported)
    ));
    assert!(matches!(
        chan.set_playlist_description(&pl, "新描述").await,
        Err(Error::NotSupported)
    ));
    Ok(())
}

#[tokio::test]
async fn artist_albums_defaults_to_not_supported() -> color_eyre::Result<()> {
    let chan = BareChannel;
    let artist = ArtistId::new(SourceKind::LOCAL, "ar-1");
    assert!(matches!(
        chan.artist_albums(&artist, Page::default()).await,
        Err(Error::NotSupported)
    ));
    Ok(())
}

#[tokio::test]
async fn caps_reports_declared_abilities() -> color_eyre::Result<()> {
    let chan = BareChannel;
    let caps = chan.caps();
    assert!(caps.searchable().is_empty());
    assert!(!*caps.playlist_edit());
    // SearchKind 仍是 caps 词汇表的一部分(编译期使用以防止意外脱钩)
    let _kinds = [SearchKind::Song];
    Ok(())
}

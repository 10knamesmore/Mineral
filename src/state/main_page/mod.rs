use album::AlbumListState;
use artist::ArtistListState;
use playlist::PlayListState;
use ratatui::widgets::Row;

pub(crate) mod album;
pub(crate) mod artist;
pub(crate) mod playlist;

pub(crate) struct MainPageState {
    pub(crate) tab: MainPageTab,
}

pub(crate) enum MainPageTab {
    PlayList(PlayListState),
    FavoriteAlbum(AlbumListState),
    FavoriteArtist(ArtistListState),
}

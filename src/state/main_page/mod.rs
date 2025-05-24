use album::AlbumListState;
use artist::ArtistListState;
use playlist::PlayListState;
use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Cell, Row},
};

use crate::util::format::format_duration;

use super::song::SongList;

pub(crate) mod album;
pub(crate) mod artist;
pub(crate) mod playlist;

pub(crate) struct MainPageState {
    pub(crate) now_tab: MainPageTab,
}

pub(crate) enum MainPageTab {
    PlayList(PlayListState),
    FavoriteAlbum(AlbumListState),
    FavoriteArtist(ArtistListState),
}

impl MainPageTab {
    pub(crate) fn get_selected_detail(&self) -> Vec<Row> {
        match self {
            MainPageTab::PlayList(state) => match state.selected {
                Some(index) => {
                    let selected_list = state
                        .items
                        .get(index)
                        .expect("Selected index out of bounds");
                    Self::get_detail_from_state(selected_list)
                }
                None => Vec::new(),
            },

            MainPageTab::FavoriteAlbum(state) => match state.selected {
                Some(index) => {
                    let selected_list = state
                        .items
                        .get(index)
                        .expect("Selected index out of bounds");
                    Self::get_detail_from_state(selected_list)
                }
                None => Vec::new(),
            },
            MainPageTab::FavoriteArtist(state) => match state.selected {
                Some(index) => {
                    let selected_list = state
                        .items
                        .get(index)
                        .expect("Selected index out of bounds");
                    Self::get_detail_from_state(selected_list)
                }
                None => Vec::new(),
            },
        }
    }

    fn get_detail_from_state<T>(state: &T) -> Vec<Row>
    where
        T: SongList,
    {
        let songs_cell: Vec<Row> = state
            .get_song_list()
            .iter()
            .enumerate()
            .map(|(i, song)| {
                let index = Text::from(Line::from(Span::styled(
                    format!("{:>2}", i + 1),
                    Style::default().bold(),
                )));

                let song_name = Text::from(Line::from(Span::styled(
                    &song.name,
                    Style::default().fg(Color::LightBlue),
                )));

                let duration = Text::from(Line::from(Span::styled(
                    format_duration(song.duration),
                    Style::default().fg(Color::Gray),
                )));

                Row::new(vec![
                    Cell::from(index),
                    Cell::from(song_name).style(Style::default()),
                    Cell::from(duration).style(Style::default()),
                ])
            })
            .collect();
        songs_cell
    }

    pub(crate) fn get_selected_id(&self) -> Option<u64> {
        match self {
            MainPageTab::PlayList(state) => state
                .selected
                .and_then(|index| state.items.get(index).map(|playlist| playlist.id)),
            MainPageTab::FavoriteAlbum(state) => state
                .selected
                .and_then(|index| state.items.get(index).map(|album| album.id)),
            MainPageTab::FavoriteArtist(state) => state
                .selected
                .and_then(|index| state.items.get(index).map(|artist| artist.id)),
        }
    }
}

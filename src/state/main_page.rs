use album::Album;
use artist::Artist;
use playlist::PlayList;
use ratatui::{
    layout::Alignment,
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Cell, Row},
};

use crate::util::format::format_duration;

use super::{Selectable, SongList};

pub(crate) mod album;
pub(crate) mod artist;
pub(crate) mod playlist;

#[derive(Debug, Default)]
pub(crate) struct MainPageState {
    pub(crate) now_tab: MainPageTab,
    playlist_state: TabList<PlayList>,
    album_state: TabList<Album>,
    artist_state: TabList<Artist>,
}

#[derive(Debug, Default)]
pub(crate) struct TabList<T> {
    items: Vec<T>,
    selected: Option<usize>,
}

impl<T> TabList<T> {
    pub(crate) fn new(items: Vec<T>) -> Self {
        Self {
            selected: if items.is_empty() { None } else { Some(0) },
            items,
        }
    }

    pub(crate) fn to_rows<'a>(&'a self) -> Vec<Row<'a>>
    where
        Row<'a>: From<&'a T>, // 这里声明 Row 可以从 &T 转换
    {
        self.items.iter().map(Row::from).collect()
    }

    pub(crate) fn selected_index(&self) -> Option<usize> {
        self.selected
    }
}

impl<T> Selectable for TabList<T> {
    type Item = T;

    fn items(&self) -> &[Self::Item] {
        &self.items
    }

    fn selected_index(&self) -> Option<usize> {
        self.selected
    }

    fn select(&mut self, index: usize) {
        self.selected = Some(index);
    }
}

#[derive(Default, Debug)]
pub(crate) enum MainPageTab {
    #[default]
    PlayList,
    FavoriteAlbum,
    FavoriteArtist,
}

impl MainPageState {
    pub(crate) fn new(playlists: Vec<PlayList>, albums: Vec<Album>, artists: Vec<Artist>) -> Self {
        Self {
            now_tab: MainPageTab::default(),
            playlist_state: TabList::new(playlists),
            album_state: TabList::new(albums),
            artist_state: TabList::new(artists),
        }
    }

    /// 根据目前的 tab 以及对应的 selected index 返回SongList
    pub(crate) fn get_song_list(&self) -> Option<&dyn SongList> {
        match self.now_tab {
            MainPageTab::PlayList => {
                let index = self.playlist_state.selected?;
                Some(&self.playlist_state.items[index])
            }
            MainPageTab::FavoriteAlbum => {
                let index = self.album_state.selected?;
                Some(&self.album_state.items[index])
            }
            MainPageTab::FavoriteArtist => {
                let index = self.artist_state.selected?;
                Some(&self.artist_state.items[index])
            }
        }
    }

    /// 根据目前的 tab 返回对应的 列表(playlist,album,artist)
    pub(crate) fn get_all_song_lists(&self) -> Vec<&dyn SongList> {
        match self.now_tab {
            MainPageTab::PlayList => self
                .playlist_state
                .items
                .iter()
                .map(|p| p as &dyn SongList)
                .collect(),
            MainPageTab::FavoriteAlbum => self
                .album_state
                .items
                .iter()
                .map(|a| a as &dyn SongList)
                .collect(),
            MainPageTab::FavoriteArtist => self
                .artist_state
                .items
                .iter()
                .map(|a| a as &dyn SongList)
                .collect(),
        }
    }

    pub(crate) fn get_now_tab_items(&self) -> Vec<Row> {
        match self.now_tab {
            MainPageTab::PlayList => self.playlist_state.to_rows(),
            MainPageTab::FavoriteAlbum => self.album_state.to_rows(),
            MainPageTab::FavoriteArtist => self.artist_state.to_rows(),
        }
    }

    /// 根据目前的 Tab 返回对应的详情页应有的内容
    ///
    /// 比如当前在 Tab 为PlayList, 就根据目前的 Selected, 返回选中的 PlayList
    /// 里面歌曲组成的列表摘要
    pub(crate) fn get_selected_detail(&self) -> Vec<Row> {
        match self.now_tab {
            MainPageTab::PlayList => match self.playlist_state.selected {
                Some(index) => {
                    let selected_list = self
                        .playlist_state
                        .items
                        .get(index)
                        .expect("Selected index out of bounds");
                    Self::get_detail_from_state(selected_list)
                }
                None => Vec::new(),
            },

            MainPageTab::FavoriteAlbum => match self.artist_state.selected {
                Some(index) => {
                    let selected_list = self
                        .album_state
                        .items
                        .get(index)
                        .expect("Selected index out of bounds");
                    Self::get_detail_from_state(selected_list)
                }
                None => Vec::new(),
            },
            MainPageTab::FavoriteArtist => match self.artist_state.selected {
                Some(index) => {
                    let selected_list = self
                        .artist_state
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
                let index = Text::from(
                    Line::from(Span::styled(
                        format!("{:>2}", i + 1),
                        Style::default().bold(),
                    ))
                    .alignment(Alignment::Left),
                );

                let song_name = Text::from(
                    Line::from(Span::styled(
                        &song.name,
                        Style::default().fg(Color::LightBlue),
                    ))
                    .alignment(Alignment::Center),
                );

                let duration = Text::from(
                    Line::from(Span::styled(
                        format_duration(song.duration),
                        Style::default().fg(Color::Gray),
                    ))
                    .alignment(Alignment::Right),
                );

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
        match self.now_tab {
            MainPageTab::PlayList => self.playlist_state.selected.and_then(|index| {
                self.playlist_state
                    .items
                    .get(index)
                    .map(|playlist| playlist.id)
            }),
            MainPageTab::FavoriteAlbum => self
                .album_state
                .selected
                .and_then(|index| self.album_state.items.get(index).map(|album| album.id)),
            MainPageTab::FavoriteArtist => self
                .artist_state
                .selected
                .and_then(|index| self.artist_state.items.get(index).map(|artist| artist.id)),
        }
    }

    pub(crate) fn playlist_state(&self) -> &TabList<PlayList> {
        &self.playlist_state
    }

    pub(crate) fn album_state(&self) -> &TabList<Album> {
        &self.album_state
    }

    pub(crate) fn artist_state(&self) -> &TabList<Artist> {
        &self.artist_state
    }

    pub(crate) fn now_tab_move_up_by(&mut self, n: usize) {
        match self.now_tab {
            MainPageTab::PlayList => self.playlist_state.move_up_by(n),
            MainPageTab::FavoriteAlbum => self.album_state.move_up_by(n),
            MainPageTab::FavoriteArtist => self.artist_state.move_up_by(n),
        }
    }

    pub(crate) fn now_tab_move_down_by(&mut self, n: usize) {
        match self.now_tab {
            MainPageTab::PlayList => self.playlist_state.move_down_by(n),
            MainPageTab::FavoriteAlbum => self.album_state.move_down_by(n),
            MainPageTab::FavoriteArtist => self.artist_state.move_down_by(n),
        }
    }
}

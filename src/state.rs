use std::fmt::Debug;

use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Text},
    widgets::{Cell, Row},
};

use crate::util::format::format_duration;

pub(crate) mod main_page;

#[derive(Clone, Copy)]
pub(crate) enum Page {
    Main,
    Search,
    // TODO
}

#[derive(Clone, Copy)]
pub(crate) enum PopupState {
    None,
    ConfirmExit,
    Notificacion, // TODO
}

#[derive(Debug, Clone)]
pub struct Song {
    pub id: u64,
    pub name: String,
    pub artist: String,
    pub album: String,
    pub duration: u32, // 秒
}

impl HasId for Song {
    fn id(&self) -> u64 {
        self.id
    }
}

pub trait SongList: Debug {
    fn get_song_list(&self) -> &[Song];
}

pub trait HasId {
    fn id(&self) -> u64;
}

impl<'a> From<&'a Song> for Row<'a> {
    fn from(song: &'a Song) -> Self {
        let text_style = Style::default().fg(Color::DarkGray).bold();

        let name_block = Text::from(vec![Line::styled(&song.name, text_style.clone())]);

        let artist_block = Text::from(vec![Line::styled(&song.name, text_style.clone())]);

        let album_block = Text::from(vec![Line::styled(&song.album, text_style.clone())]);

        let duration_block = Text::from(vec![Line::styled(
            format_duration(song.duration),
            text_style,
        )]);

        Row::new(vec![
            Cell::from(name_block),
            Cell::from(artist_block),
            Cell::from(album_block),
            Cell::from(duration_block),
        ])
    }
}

#[allow(dead_code)]
pub(crate) trait Selectable {
    type Item: HasId;

    fn items(&self) -> &[Self::Item];
    fn selected_index(&self) -> Option<usize>;
    fn _select(&mut self, index: usize);

    fn len(&self) -> usize {
        self.items().len()
    }
    fn is_empty(&self) -> bool {
        self.items().is_empty()
    }

    fn selected_item(&self) -> Option<&Self::Item> {
        self.selected_index()
            .and_then(|index| self.items().get(index))
    }

    fn move_up(&mut self) {
        self.move_up_by(1);
    }
    fn move_down(&mut self) {
        self.move_down_by(1);
    }

    fn move_up_by(&mut self, n: usize) {
        if let Some(index) = self.selected_index() {
            if index >= n {
                self._select(index - n);
            } else {
                self._select(0);
            }
        }
    }
    fn move_down_by(&mut self, n: usize) {
        let items = self.items();
        if let Some(index) = self.selected_index() {
            if index + n < items.len() {
                self._select(index + n);
            } else if !items.is_empty() {
                self._select(items.len() - 1);
            }
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct Introduction {
    description: String,
}

impl Introduction {
    pub(crate) fn new(description: String) -> Introduction {
        Introduction { description }
    }

    pub(crate) fn desc(&self) -> &str {
        &self.description
    }
}

pub(crate) trait HasIntroduction {
    fn introduction(&self) -> &Introduction;
}

use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Cell, Row},
};
use std::vec;

use crate::state::selectable::Selectable;

pub(crate) struct AlbumListState {
    pub(crate) items: Vec<Album>,
    pub(crate) selected: Option<usize>,
}

pub(crate) struct Album {
    pub(crate) artist: String,
    pub(crate) cover_path: String,
    pub(crate) song_num: u32,
    pub(crate) title: String,
    pub(crate) year: u32,
}

impl Selectable for AlbumListState {
    type Item = Album;
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

impl<'a> From<&'a Album> for Row<'a> {
    fn from(album: &'a Album) -> Self {
        let left = Text::from(vec![
            Line::from(vec![Span::styled(
                format!("《{}》 ({})", album.title, album.year),
                Style::default().fg(Color::DarkGray).bold(),
            )]),
            Line::from(Span::raw(&album.artist)),
        ]);

        let minutes = 99; // TODO

        let right = Text::from(Line::from(vec![Span::styled(
            format!("{} 首 · {}min", &album.song_num, minutes),
            Style::default().fg(Color::LightBlue),
        )]));

        Row::new(vec![
            Cell::from(left),
            Cell::from(right).style(Style::default()),
        ])
    }
}

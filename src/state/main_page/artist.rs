use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Cell, Row},
};

use crate::state::{
    selectable::Selectable,
    song::{Song, SongList},
};

pub(crate) struct ArtistListState {
    pub(crate) items: Vec<Artist>,
    pub(crate) selected: Option<usize>,
}

pub(crate) struct Artist {
    pub(crate) name: String,
    pub(crate) followers: u32,
    pub(crate) cover_path: String,
    pub(crate) songs: Vec<Song>,
    pub(crate) id: u64,
}

impl Selectable for ArtistListState {
    type Item = Artist;
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

fn format_follower_count(followers: u32) -> String {
    match followers {
        n if n >= 1_000_000 => format!("{:.2}M", n as f64 / 1_000_000.0),
        n if n >= 1_000 => format!("{:.1}K", n as f64 / 1_000.0),
        n => format!("{}", n),
    }
}

impl<'a> From<&'a Artist> for Row<'a> {
    fn from(artist: &'a Artist) -> Self {
        let name_cell = Cell::from(Line::from(Span::styled(
            &artist.name,
            Style::default().fg(Color::LightYellow).bold(),
        )));

        let followers_formatted = format!("{:>}", format_follower_count(artist.followers));
        let followers_cell = Cell::from(Line::from(Span::styled(
            followers_formatted,
            Style::default().fg(Color::LightBlue),
        )));

        Row::new(vec![name_cell, followers_cell])
    }
}

impl SongList for Artist {
    fn get_song_list(&self) -> &[Song] {
        &self.songs
    }
}

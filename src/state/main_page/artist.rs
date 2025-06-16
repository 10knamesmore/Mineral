use crate::state::{HasId, HasIntroduction, Introduction, Song, SongList};
use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{Cell, Row},
};

#[derive(Debug, Default)]
pub(crate) struct Artist {
    pub(crate) name: String,
    pub(crate) followers: u32,
    pub(crate) songs: Vec<Song>,
    pub(crate) id: u64,
    introduction: Introduction,
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

impl HasId for Artist {
    fn id(&self) -> u64 {
        self.id
    }
}

impl SongList for Artist {
    fn songs(&self) -> &[Song] {
        &self.songs
    }
}

impl HasIntroduction for Artist {
    fn introduction(&self) -> &Introduction {
        &self.introduction
    }
}

impl Artist {
    pub(crate) fn to_rows(&self) -> Vec<Row<'_>> {
        // HACK: 需要优化Artist的显示
        self.songs.iter().map(|song| song.into()).collect()
    }
}

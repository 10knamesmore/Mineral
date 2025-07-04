use crate::{
    app::Song,
    state::{HasDescription, HasId, SongList},
    util::format::format_duration,
};
use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Cell, Row},
};
use std::vec;

#[derive(Debug)]
pub(crate) struct Album {
    pub(crate) id: u64,
    pub(crate) name: String,

    pub(crate) artist_id: u64,
    pub(crate) artist_name: String,

    pub(crate) description: String,
    pub(crate) publish_time: u64,

    pub(crate) pic_url: String,

    pub(crate) songs: Vec<Song>,
}

impl<'a> From<&'a Album> for Row<'a> {
    fn from(album: &'a Album) -> Self {
        let left = Text::from(vec![
            Line::from(vec![Span::styled(
                format!("《{}》 ({})", album.name, album.publish_time),
                Style::default().fg(Color::DarkGray).bold(),
            )]),
            Line::from(Span::raw(&album.artist_name)),
        ]);

        let duration: u64 = album.songs.iter().map(|song| song.duration).sum();

        let right = Text::from(Line::from(vec![Span::styled(
            format!("{} 首 · {}", &album.songs.len(), format_duration(duration)),
            Style::default().fg(Color::LightBlue),
        )]));

        Row::new(vec![
            Cell::from(left),
            Cell::from(right).style(Style::default()),
        ])
    }
}

impl HasId for Album {
    fn id(&self) -> u64 {
        self.id
    }
}

impl SongList for Album {
    fn songs(&self) -> &[Song] {
        &self.songs
    }
}

impl HasDescription for Album {
    fn description(&self) -> &str {
        &self.description
    }
}

impl Album {
    pub(crate) fn to_rows(&self) -> Vec<Row> {
        self.songs.iter().map(|song| song.into()).collect()
    }
}

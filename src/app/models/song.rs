use crate::{state::HasId, util::format::format_duration};
use std::fmt::Debug;

use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Text},
    widgets::{Cell, Row},
};

#[derive(Debug, Clone)]
pub struct Song {
    pub id: u64,

    pub name: String,

    // TODO: 对多名artists的支持
    pub artist_id: u64,
    pub artist_name: String,

    pub album_id: u64,
    pub album_name: String,

    pub pic_url: String,

    pub song_url: String,

    pub duration: u64, // 秒
}

impl HasId for Song {
    fn id(&self) -> u64 {
        self.id
    }
}

impl<'a> From<&'a Song> for Row<'a> {
    fn from(song: &'a Song) -> Self {
        let text_style = Style::default().fg(Color::DarkGray).bold();

        let name_block = Text::from(vec![Line::styled(&song.name, text_style.clone())]);

        let artist_block = Text::from(vec![Line::styled(&song.name, text_style.clone())]);

        let album_block = Text::from(vec![Line::styled(&song.album_name, text_style.clone())]);

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

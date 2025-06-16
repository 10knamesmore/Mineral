use crate::state::{HasId, HasIntroduction, Introduction, Song, SongList};
use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Cell, Row},
};

#[derive(Debug, Default)]
pub(crate) struct PlayList {
    pub(crate) name: String,
    pub(crate) track_count: usize,
    pub(crate) songs: Vec<Song>,
    pub(crate) id: u64,
    pub(crate) introduction: Introduction,
}

impl<'a> From<&'a PlayList> for Row<'a> {
    fn from(play_list: &'a PlayList) -> Self {
        let left = Text::from(Line::from(Span::styled(
            &play_list.name,
            Style::default().bold(),
        )));

        let right = Text::from(Line::from(Span::styled(
            format!("共 {} 首", &play_list.track_count),
            Style::default().fg(Color::LightBlue),
        )));

        Row::new(vec![
            Cell::from(left),
            Cell::from(right).style(Style::default()),
        ])
    }
}

impl HasId for PlayList {
    fn id(&self) -> u64 {
        self.id
    }
}

impl SongList for PlayList {
    fn songs(&self) -> &[Song] {
        &self.songs
    }
}

impl HasIntroduction for PlayList {
    fn introduction(&self) -> &Introduction {
        &self.introduction
    }
}

impl PlayList {
    pub(crate) fn to_rows<'a>(&'a self) -> Vec<Row<'a>> {
        self.songs.iter().map(|song| song.into()).collect()
    }
}

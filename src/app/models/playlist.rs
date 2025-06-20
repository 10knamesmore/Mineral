use crate::{
    app::Song,
    state::{HasDescription, HasId, SongList},
};
use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Cell, Row},
};

#[derive(Debug, Default)]
pub(crate) struct PlayList {
    pub(crate) id: u64,
    pub(crate) name: String,

    pub(crate) img_url: String,

    pub(crate) track_count: u64,

    pub(crate) songs: Vec<Song>,
    pub(crate) description: String,
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

impl HasDescription for PlayList {
    fn description(&self) -> &str {
        &self.description
    }
}

impl PlayList {
    pub(crate) fn to_rows(&self) -> Vec<Row> {
        self.songs.iter().map(|song| song.into()).collect()
    }
}

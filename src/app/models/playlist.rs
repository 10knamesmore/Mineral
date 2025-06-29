use std::{fs::DirEntry, path::PathBuf};

use crate::{
    app::Song,
    state::{HasDescription, HasId, SongList},
    util::format::format_duration,
};
use anyhow::Context;
use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Cell, Row},
};

#[derive(Debug)]
pub(crate) struct PlayList {
    pub local: bool,

    pub(crate) id: u64,
    pub(crate) name: String,

    // TODO: 改为Option
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

        let duration: u64 = play_list.songs.iter().map(|song| song.duration).sum();

        let right = Text::from(Line::from(Span::styled(
            format!(
                "共 {} 首 · {}",
                &play_list.track_count,
                format_duration(duration)
            ),
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

    pub fn from_path(path: &PathBuf) -> anyhow::Result<PlayList> {
        use crate::util::fs;

        let name = fs::name_from_path(path);
        let id = fs::hash_path(path);

        let songs: Vec<Song> = if path.is_dir() {
            std::fs::read_dir(path)?
                .filter_map(Result::ok)
                .filter(|f| f.path().is_file())
                .map(|f| f.path())
                .filter_map(|f| Song::from_path(&f).ok())
                .collect()
        } else {
            anyhow::bail!("{:?} 不是一个目录", path);
        };
        let track_count = songs.len() as u64;

        Ok(PlayList {
            local: true,
            id,
            name,
            img_url: String::new(),
            track_count,
            songs,
            description: String::new(),
        })
    }
}

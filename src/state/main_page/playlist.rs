use ratatui::{
    style::{Color, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Cell, Row},
};

use crate::state::{selectable::Selectable, song::Song};

pub(crate) struct PlayListState {
    pub(crate) items: Vec<PlayList>,    // 列表数据
    pub(crate) selected: Option<usize>, // 当前选中项
}

pub(crate) struct PlayList {
    pub(crate) name: String,
    pub(crate) track_count: usize,
    pub(crate) cover_path: String,
    pub(crate) songs: Vec<Song>,
    // 可加 id, 创建时间等
}

impl Selectable for PlayListState {
    type Item = PlayList;
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

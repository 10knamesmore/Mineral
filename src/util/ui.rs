use ratatui::{style::Style, widgets::Row};

use crate::app::TableColors;

pub(crate) fn zebra_rows<'a>(items: Vec<Row<'a>>, colors: &TableColors) -> Vec<Row<'a>> {
    items
        .into_iter()
        .enumerate()
        .map(|(index, row)| {
            let bg_color = if index % 2 == 0 {
                colors.normal_row_color
            } else {
                colors.alt_row_color
            };
            row.height(2).style(Style::default().bg(bg_color))
        })
        .collect()
}

use ratatui::{style::Style, widgets::Row};

use crate::app::TableColors;

pub(crate) fn zebra_rows<'a, T>(items: &'a [T], colors: &TableColors) -> Vec<Row<'a>>
where
    &'a T: Into<Row<'a>>,
{
    items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let row: Row = item.into();
            let bg_color = if index % 2 == 0 {
                colors.normal_row_color
            } else {
                colors.alt_row_color
            };
            row.height(2).style(Style::default().bg(bg_color))
        })
        .collect()
}

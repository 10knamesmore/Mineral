use ratatui::{
    Frame, layout,
    style::{Color, Style},
    widgets::{Block, BorderType, Borders},
};

use crate::App;

pub(super) fn render_playback_control(app: &App, frame: &mut Frame, area: layout::Rect) {
    let block = Block::default()
        .title("Playback Control")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    frame.render_widget(block, area);
}

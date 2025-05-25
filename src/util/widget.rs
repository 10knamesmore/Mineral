use derive_setters::Setters;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap},
};

#[derive(Debug, Default, Setters)]
pub(crate) struct Popup<'a> {
    #[setters(into)]
    title: Line<'a>,
    #[setters(into)]
    content: Text<'a>,
    border_style: Style,
    title_style: Style,
    style: Style,
}

impl<'a> Popup<'a> {
    pub fn get_title(&self) -> &Line<'a> {
        &self.title
    }

    pub fn get_content(&self) -> &Text<'a> {
        &self.content
    }

    pub fn get_border_style(&self) -> &Style {
        &self.border_style
    }

    pub fn get_title_style(&self) -> &Style {
        &self.title_style
    }

    pub fn get_style(&self) -> &Style {
        &self.style
    }
}

impl Widget for Popup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        // ensure that all cells under the popup are cleared to avoid leaking content
        Clear.render(area, buf);
        let block = Block::new()
            .title(self.title)
            .title_style(self.title_style)
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(self.border_style);
        Paragraph::new(self.content)
            .wrap(Wrap { trim: true })
            .style(self.style)
            .block(block)
            .render(area, buf);
    }
}

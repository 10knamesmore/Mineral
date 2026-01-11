use ratatui::style::Color;

pub(crate) struct TableColors {
    pub(crate) buffer_bg: Color,
    pub(crate) row_fg: Color,
    pub(crate) selected_row_style_fg: Color,
    pub(crate) normal_row_color: Color,
    pub(crate) alt_row_color: Color,
}

impl Default for TableColors {
    fn default() -> Self {
        TableColors {
            buffer_bg: Color::Black,
            row_fg: Color::White,
            selected_row_style_fg: Color::Yellow,
            normal_row_color: Color::Gray,
            alt_row_color: Color::DarkGray,
        }
    }
}

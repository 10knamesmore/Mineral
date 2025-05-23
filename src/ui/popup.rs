use crate::util::{layout::center, widget::Popup};
use ratatui::{
    Frame,
    layout::Constraint,
    style::{Style, Stylize},
    text::{Line, Span, Text},
};

pub(super) fn popup_confirm_exit(frame: &mut Frame) {
    let area = center(
        frame.area(),
        Constraint::Percentage(20),
        Constraint::Length(3),
    );

    let content = Text::from(vec![
        Line::from("(y) for yes, or (n) for no"),
        Line::from(vec![
            Span::styled("[y]", Style::default().green().bold()),
            Span::raw("   "),
            Span::styled("[n]", Style::default().red().bold()),
        ]),
    ]);

    let popup = Popup::default()
        .content(content)
        .title("是否确定退出")
        .title_style(Style::new().white().bold())
        .border_style(Style::new().red());

    frame.render_widget(popup, area);
}

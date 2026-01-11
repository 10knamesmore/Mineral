use crate::util::{
    layout::{center, top_center},
    notification::{Notification, NotifyUrgency},
    widget::Popup,
};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Alignment, Constraint, Rect},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::Widget,
};

pub(super) fn confirm_exit(frame: &mut Frame) {
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
        .title(Line::from("是否确定退出").style(Style::new().white().bold()))
        .border_style(Style::new().red());

    frame.render_widget(popup, area);
}

pub(super) fn notify(notification: &Notification, frame: &mut Frame) {
    let area = top_center(
        frame.area(),
        Constraint::Percentage(20),
        Constraint::Length(3),
    );

    frame.render_widget(notification, area);
}

impl Widget for &Notification {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let noti_title = match self.urgency() {
            NotifyUrgency::Debug => format!("󰋽 DEBUG : {}", self.title()),
            NotifyUrgency::Info => format!(" INFO : {}", self.title()),
            NotifyUrgency::Warning => format!(" WARNING : {}", self.title()),
            NotifyUrgency::Error => format!(" ERROR : {}", self.title()),
        };

        let urgency_color = match self.urgency() {
            NotifyUrgency::Debug => Color::Gray,
            NotifyUrgency::Info => Color::Blue,
            NotifyUrgency::Warning => Color::Yellow,
            NotifyUrgency::Error => Color::Red,
        };

        // 构造 Popup 使用的 style，允许 Notification 的样式优先
        let popup = Popup::default()
            .title(
                Line::from(noti_title).alignment(Alignment::Center).style(
                    Style::default()
                        .fg(urgency_color)
                        .add_modifier(Modifier::BOLD),
                ),
            )
            .content(Text::from(self.content().as_str()).style(Style::default().fg(urgency_color)))
            .border_style(Style::default().fg(urgency_color));

        popup.render(area, buf);
    }
}

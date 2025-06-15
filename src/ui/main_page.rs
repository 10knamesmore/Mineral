use super::components::render_playback_control;
use crate::{
    app::{App, RenderCache},
    util::{layout::center, ui::zebra_rows},
};
use ratatui::{
    Frame,
    layout::{self, Constraint, Direction, Layout, Margin},
    style::{Color, Modifier, Style, Stylize},
    text::Text,
    widgets::{Block, BorderType, Borders, HighlightSpacing, Table, TableState},
};
use ratatui_image::StatefulImage;

pub fn draw_main_page(app: &App, frame: &mut Frame, cache: &mut RenderCache) {
    // TEMP
    let [detail_area, playback_control_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Fill(10), Constraint::Length(8)])
        .areas(frame.area());
    let [table_area, detail_area] = Layout::default()
        .direction(layout::Direction::Horizontal)
        .constraints([Constraint::Percentage(80), Constraint::Percentage(20)])
        .areas(detail_area);

    render_table(app, frame, table_area);
    render_detail(app, frame, detail_area, cache);
    render_playback_control(app, frame, playback_control_area);
}

fn render_table(app: &App, frame: &mut Frame, area: layout::Rect) {
    let rows = zebra_rows(app.get_main_tab_items_as_row(), &app.colors);

    let table = Table::default()
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" 播放列表 ")
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_symbol("▶ ") // 高亮行前缀
        .row_highlight_style(
            Style::default()
                .bg(Color::Blue)
                .fg(Color::Black)
                .add_modifier(Modifier::REVERSED),
        )
        .rows(rows)
        .widths(vec![Constraint::Percentage(60), Constraint::Percentage(40)]);

    let mut table_state = TableState::default().with_selected(app.get_main_tab_selected_index());

    frame.render_stateful_widget(table, area, &mut table_state);
}

fn render_detail(app: &App, frame: &mut Frame, area: layout::Rect, cache: &mut RenderCache) {
    let block = Block::default()
        .title("Detail")
        .bg(Color::Black)
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Yellow));

    frame.render_widget(block, area);

    let [cover_area, list_area] = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![
            Constraint::Length(area.width / 3),
            Constraint::Fill(1),
        ])
        .areas(area);

    // 封面渲染
    let cover_area = center(
        cover_area.inner(Margin::new(3, 1)),
        Constraint::Percentage(100),
        Constraint::Percentage(100),
    );
    // frame.render_widget(Block::default().bg(Color::Blue), cover_area);
    if let Some(id) = app.main_page().get_selected_id() {
        let tried_cached_image = app.main_page().get_now_cover(cache);

        match tried_cached_image {
            crate::app::ImageState::NotRequested => {} // MainPageState 的 selected_idx 为 None ,这时候不该渲染cover
            crate::app::ImageState::Loading => {
                // HACK: 优化正在时的表现
                let place_holder_text = Text::from("图片加载中...");
                frame.render_widget(place_holder_text, cover_area);
            }
            crate::app::ImageState::Loaded(cached_image) => {
                frame.render_stateful_widget(
                    StatefulImage::default(),
                    area,
                    &mut *cached_image.borrow_mut(),
                );
            }
            crate::app::ImageState::Failed(e) => {
                // HACK: 优化加载失败时的错误提醒
                let place_holder_text = Text::from(format!("图片加载失败: {}", e));
                frame.render_widget(place_holder_text, cover_area);
            }
        }
    };

    // 歌曲摘要渲染
    if let Some(detail_widget) = app.get_selected_detail() {
        frame.render_widget(detail_widget, list_area.inner(Margin::new(1, 1)))
    }
}

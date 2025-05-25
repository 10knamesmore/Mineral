use super::components::render_playback_control;
use crate::app::{App, RenderCache};
use crate::state::main_page::MainPageTab;
use crate::util::layout::{aspect_fit_center, center};
use crate::util::ui::zebra_rows;
use ratatui::layout::Margin;
use ratatui::style::Stylize;
use ratatui::{
    Frame,
    layout::{self, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    widgets::{Block, BorderType, Borders, HighlightSpacing, Row, Table, TableState},
};
use ratatui_image::{StatefulImage, picker::Picker};

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
    let rows = match &app.main_page().now_tab {
        MainPageTab::PlayList(state) => zebra_rows(&state.items, &app.colors),
        MainPageTab::FavoriteAlbum(state) => zebra_rows(&state.items, &app.colors),
        MainPageTab::FavoriteArtist(state) => zebra_rows(&state.items, &app.colors),
    };

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
    let now_tab = &app.main_page().now_tab;
    if let Some(id) = now_tab.get_selected_id() {
        let tried_cached_image = match now_tab {
            MainPageTab::PlayList(_) => cache.get_playlist_cover(id),
            MainPageTab::FavoriteAlbum(_) => cache.get_album_cover(id),
            MainPageTab::FavoriteArtist(_) => cache.get_artist_cover(id),
        };
        if let Some(cached_image) = tried_cached_image {
            // 如果缓存中有图片，直接使用
            frame.render_stateful_widget(StatefulImage::default(), cover_area, cached_image);
        }
    };

    // 歌曲摘要渲染
    let rows = app.get_selected_detail();
    let table = Table::default()
        .rows(rows)
        .block(
            Block::default()
                .title(" 歌曲列表 ")
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Yellow)),
        )
        .widths(vec![
            Constraint::Percentage(20),
            Constraint::Percentage(40),
            Constraint::Percentage(40),
        ]);
    frame.render_widget(table, list_area.inner(Margin::new(1, 1)));
}

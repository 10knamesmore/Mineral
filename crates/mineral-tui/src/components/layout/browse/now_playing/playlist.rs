//! Playlists 视图右栏:程序化封面 + 歌单名/meta 两行(居中) + 底部简介行(空显占位)。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui_image::picker::Picker;

use crate::components::layout::shared::cover_image;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;
use crate::runtime::view_model::PlaylistView;

/// 渲染歌单详情(right pane)到 `area`。
///
/// # Params:
///   - `cover_in_flight`: page morph 封面飞行层已接管主封面时置真——跳过自画封面防双画
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    p: &PlaylistView,
    state: &AppState,
    picker: &Picker,
    theme: &Theme,
    cover_in_flight: bool,
) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" selected ").style(Style::new().fg(theme.subtext)));
    frame.render_widget(block, area);
    let Some([cover_area, kv_area, footer]) = super::main_cover::sections(area) else {
        return;
    };

    if !cover_in_flight {
        // mineral 聚合歌单无自带封面:拼贴就绪时给合成键,未就绪回落程序化占位。
        let cover = crate::runtime::cover::collage::effective_cover_url(state, &p.data);
        cover_image::render_or_fallback(
            frame,
            cover_area,
            cover.as_ref(),
            state,
            picker,
            theme,
            &p.data.name,
        );
        // 暖入口曲封面:drill 进本歌单默认落到第 0 首,其封面在 Library 视图才首次显示。
        // 悬停期就按封面区尺寸(两视图同几何)提前编码协议,使 drill 瞬间直接命中 kitty、
        // 不闪 hash→halfblock。图未预取到 cache 时 prewarm 无操作(fetch 侧负责先拉进来)。
        if let Some(first) = state.library.tracks.get(&p.data.id).and_then(|t| t.first())
            && let Some(url) = first.data.cover_url.as_ref()
        {
            cover_image::prewarm(state, picker, cover_area, url);
        }
    }

    let total_ms = state.total_duration_ms_of(&p.data.id);
    let len_label = if total_ms == 0 {
        String::from("—")
    } else {
        let total_min = total_ms / 60_000;
        format!("{}h {:02}m", total_min / 60, total_min % 60)
    };

    let src = p.data.source();
    // 标题行:歌单名(text + bold);meta 行:源(源色)· tracks · 总时长(overlay)。居中。
    let kv = vec![
        Line::from(Span::styled(
            p.data.name.clone(),
            Style::new().fg(theme.text).add_modifier(Modifier::BOLD),
        )),
        Line::from(vec![
            Span::styled(
                src.label(),
                Style::new().fg(crate::render::theme::resolve_source_color(
                    theme,
                    state.cfg.sources(),
                    src,
                )),
            ),
            Span::styled(
                format!(" · {} tracks · {len_label}", p.data.track_count),
                Style::new().fg(theme.overlay),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(kv).alignment(Alignment::Center), kv_area);

    // 底行:歌单简介首个非空行(overlay,居中截断);无简介显占位——详情面板不放按键
    // 提示(发现交给 ? 帮助浮层),占位语义与 no match found 同款措辞。
    let desc = p
        .data
        .description
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty());
    let footer_line = match desc {
        Some(d) => Line::from(Span::styled(d, Style::new().fg(theme.overlay))),
        None => Line::from(Span::styled(
            "no description",
            Style::new().fg(theme.overlay).add_modifier(Modifier::DIM),
        )),
    };
    frame.render_widget(
        Paragraph::new(footer_line).alignment(Alignment::Center),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_model::{MediaUrl, PlaylistId, SourceKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    use crate::runtime::view_model::SongView;
    use crate::test_support::{app_with_playlists_probed, song};

    /// Playlists 视图悬停选中歌单、入口曲(第 0 首)封面已在 `covers.cache`:渲染右栏时
    /// 应按封面区尺寸提前编码该曲协议(`encode_pending` 落一条),使 drill 进 tracks 瞬间
    /// 直接命中 kitty、不闪 hash→halfblock。歌单本身无封面(程序化占位、不编码),故
    /// pending 里唯一的条目就是入口曲封面——反证预热的正是入口曲。
    #[test]
    fn playlist_detail_prewarms_entry_track_cover() -> color_eyre::Result<()> {
        let (mut app, _tasks) = app_with_playlists_probed()?;
        let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
        let url = MediaUrl::remote("https://x.y/entry.jpg")?;
        let mut entry = song("s0");
        entry.cover_url = Some(url.clone());
        app.state.library.tracks.insert(
            pid,
            vec![SongView {
                data: entry,
                loved: false,
                plays: None,
            }],
        );
        app.state.browse.nav.playlist.set_sel(0);
        // 入口曲图入 cache——否则 prewarm 无操作(它只对已解码在缓存的图提前编码)。
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(64, 64));
        app.state.covers.cache.insert(&url, Arc::new(img));

        let mut terminal = Terminal::new(TestBackend::new(120, 40))?;
        assert!(
            app.state.covers.encode_pending.borrow().is_empty(),
            "前置:尚未渲染,encode_pending 为空"
        );
        let p = app
            .state
            .selected_playlist()
            .ok_or_else(|| color_eyre::eyre::eyre!("应有选中歌单"))?;
        terminal.draw(|frame| {
            super::draw(
                frame,
                Rect::new(0, 0, 40, 20),
                p,
                &app.state,
                &app.picker,
                &app.theme,
                /*cover_in_flight*/ false,
            );
        })?;

        let pending = app.state.covers.encode_pending.borrow();
        assert!(
            pending.iter().any(|(u, _)| u == &url),
            "入口曲封面应被按封面区尺寸提前编码(encode_pending 应含其 URL)"
        );
        Ok(())
    }
}

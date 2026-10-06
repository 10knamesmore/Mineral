//! Playlists 视图右栏：歌单拼贴 + 歌单名/meta 两行 + 底部简介行。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use super::NowPlayingView;
use crate::image::ImageContent;
use crate::render::theme::Theme;
use crate::runtime::view_model::PlaylistView;

/// 渲染歌单详情(right pane)到 `area`。
///
/// # Params:
///   - `cover_in_flight`: page morph 封面飞行层已接管主封面时置真——跳过自画封面防双画
pub(crate) fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    p: &PlaylistView,
    state: &NowPlayingView<'_>,
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
        // Mineral 聚合歌单无自带封面：拼贴就绪时显示合成图，未就绪时留空。
        let cover = &state.input.playlist_cover;
        state.images.render(
            ImageContent::Display {
                url: cover.as_ref(),
            },
            cover_area,
            frame.buffer_mut(),
            state.phase,
        );
    }

    let len_label = state.input.duration_ms.map_or_else(
        || String::from("—"),
        |total_ms| {
            let total_min = total_ms / 60_000;
            format!("{}h {:02}m", total_min / 60, total_min % 60)
        },
    );

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
                    state.frame.config.source_colors(),
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
    use ratatui::layout::Rect;

    use crate::test_support::{app_with_playlists_probed, entry_views, song};

    /// Playlists 视图悬停选中歌单、入口曲图片已解码时，准备右栏应按封面区尺寸提前编码
    /// 该曲的终端图片，使 drill 进 tracks 后能直接命中。
    #[test]
    fn playlist_detail_prewarms_entry_track_cover() -> color_eyre::Result<()> {
        let (mut app, _tasks) = app_with_playlists_probed()?;
        let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
        let url = MediaUrl::remote("https://x.y/entry.jpg")?;
        let mut entry = song("s0");
        entry.cover_url = Some(url.clone());
        app.state.models.library.tracks.insert(
            pid,
            crate::runtime::state::PlaylistTracks {
                entries: entry_views(vec![entry]),
                complete: true,
                next_offset: None,
            },
        );
        app.state.ui.browse.playlists.select(0);
        // 入口曲图入 cache——否则 prewarm 无操作(它只对已解码在缓存的图提前编码)。
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(64, 64));
        app.state
            .resources
            .images
            .cache
            .insert_test(&url, Arc::new(img));

        assert!(
            app.state.resources.images.encode_pending.is_empty(),
            "前置:尚未准备,encode_pending 为空"
        );
        app.state.resources.images.begin_preparation();
        crate::view::now_playing::prepare(
            Rect::new(0, 0, 40, 20),
            &mut app.state,
            &app.theme,
            false,
            false,
        );
        app.state.resources.images.finish_preparation();

        let pending = app.state.resources.images.encode_pending;
        assert!(
            pending.iter().any(|key| key.matches_url(&url)),
            "入口曲封面应被按封面区尺寸提前编码(encode_pending 应含其 URL)"
        );
        Ok(())
    }
}

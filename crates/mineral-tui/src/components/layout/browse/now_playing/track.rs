//! Library 视图右栏：曲目封面 + 标题/副信息两行 + 底部 meta 行。

use mineral_model::SongId;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::components::layout::shared::marquee::MarqueeCtx;
use crate::image::ImageContent;
use crate::render::theme::Theme;
use crate::runtime::format::format_ms_opt;
use crate::runtime::marquee::Slot;
use crate::runtime::state::AppState;
use crate::runtime::view_model::PlaylistEntryView;

/// 渲染曲目详情(right pane)到 `area`。
///
/// # Params:
///   - `cover_in_flight`: page morph 封面飞行层已接管主封面时置真——跳过自画封面防双画
#[allow(clippy::too_many_arguments)] // reason: 纯渲染入口,参数即全部输入,收拢成 struct 反而多一层搬运
pub fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    entry: &PlaylistEntryView,
    current_id: Option<&SongId>,
    state: &AppState,
    theme: &Theme,
    cover_in_flight: bool,
) {
    let song = &entry.data.song;
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" selected ").style(Style::new().fg(theme.subtext)));
    frame.render_widget(block, area);
    let Some([cover_area, kv_area, current_strip]) = super::main_cover::sections(area) else {
        return;
    };

    if !cover_in_flight {
        state.images.render(
            ImageContent::Display {
                url: song.cover_url.as_ref(),
            },
            cover_area,
            frame.buffer_mut(),
            state.image_render_phase(),
        );
    }

    let is_current = current_id.is_some_and(|current| current == &song.id);

    // 标题行:歌名独占一行(text + bold,居中,长名走 marquee)。选中 =
    // 在播时整行 accent 高亮——「选中」恒正常亮度,「在播」只是标题行换色;别名**跟随整行
    // style**(playing 与歌名一起 accent、否则一起 bold),靠括号而非颜色表达次级——避免
    // 像列表那样单独染 dim 造成高亮态 bright-dim-bright 断层。
    let title_style = if is_current {
        Style::new().fg(theme.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.text).add_modifier(Modifier::BOLD)
    };
    let alias = song
        .alias
        .as_deref()
        .map(|a| format!(" ({a})"))
        .unwrap_or_default();
    let title_line = MarqueeCtx::new(state, theme, /*fade_to*/ theme.base).line(
        vec![Span::styled(format!("{}{alias}", song.name), title_style)],
        Slot::NowPlaying,
        &song.id.qualified(),
        kv_area.width,
    );

    // 副信息行:艺人 · 专辑(专辑在此首次露面;无专辑只留艺人)。
    let artist = song
        .artists
        .first()
        .map(|a| a.name.clone())
        .unwrap_or_default();
    let mut sub_spans = vec![Span::styled(artist, Style::new().fg(theme.subtext))];
    if let Some(album) = song.album.as_ref() {
        sub_spans.push(Span::styled(" · ", Style::new().fg(theme.overlay)));
        sub_spans.push(Span::styled(
            album.name.clone(),
            Style::new().fg(theme.overlay),
        ));
    }
    frame.render_widget(
        Paragraph::new(vec![title_line, Line::from(sub_spans)]).alignment(Alignment::Center),
        kv_area,
    );

    // meta 底行:时长 · ♥/♡ · Mineral 本地自然播完次数(未采集时省略),居中。
    let len = format_ms_opt(song.duration_ms);
    let mut meta_spans = vec![
        Span::styled(len, Style::new().fg(theme.overlay)),
        Span::styled(" · ", Style::new().fg(theme.overlay)),
        if entry.loved {
            Span::styled("♥", Style::new().fg(theme.red))
        } else {
            Span::styled("♡", Style::new().fg(theme.overlay))
        },
    ];
    if let Some(n) = entry.plays {
        meta_spans.push(Span::styled(" · ", Style::new().fg(theme.overlay)));
        meta_spans.push(Span::styled(
            format!("{n} plays"),
            Style::new().fg(theme.overlay),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(meta_spans)).alignment(Alignment::Center),
        current_strip,
    );
}

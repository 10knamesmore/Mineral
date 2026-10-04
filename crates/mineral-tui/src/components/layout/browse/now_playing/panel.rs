//! 右栏选中项详情：随左栏视图进度淡出淡入，排版固定；无选中项时绘制空面板。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Borders};

use super::{NowPlaying, NowPlayingInput, NowPlayingView};
use crate::components::frame::PrepareCx;
use crate::components::layout::transition;
use crate::image::ImageRenderPhase;
use crate::render::theme::Theme;
use crate::runtime::state::View;

use super::{cover_transition, playlist, track};

/// 当前选择的准备输入；飞行层接管封面时仍准备标题。
pub(crate) struct NowPlayingPreparation<'a> {
    /// 本次选择与共享模型。
    pub(crate) selection: NowPlayingInput<'a>,

    /// 页面转场是否接管主封面。
    pub(crate) cover_in_flight: bool,
}

/// 准备主封面、进入曲目的预热以及标题相位。
impl crate::components::lifecycle::Prepare for NowPlaying {
    type Input<'a> = NowPlayingPreparation<'a>;

    fn prepare(
        &mut self,
        area: Rect,
        preparation: NowPlayingPreparation<'_>,
        cx: &mut PrepareCx<'_>,
    ) {
        let input = &preparation.selection;
        let now = cx.frame.now;
        let phase = cx.image_phase;
        let images = &mut cx.images;
        let cover_in_flight = preparation.cover_in_flight;
        let Some([cover_area, kv, _]) = super::main_cover::sections(area) else {
            return;
        };
        if !input.switch.at_min()
            && let Some(entry) = input.track
        {
            self.title.prepare(
                &entry.data.song.id.qualified(),
                crate::components::layout::shared::marquee::song_title_width(&entry.data.song),
                kv.width,
                now,
            );
        }
        if cover_in_flight {
            return;
        }
        let from = input.playlist_cover.as_ref();
        let to = input
            .track
            .and_then(|entry| entry.data.song.cover_url.as_ref());
        if input.switch.at_min() || input.switch.at_max() {
            let url = if input.switch.at_min() { from } else { to };
            images.display(url, cover_area, phase);
            if input.switch.at_min()
                && let Some(url) = &input.upcoming_cover
            {
                images.prewarm(url, cover_area);
            }
        } else if match (from, to) {
            (None, None) => true,
            (Some(from), Some(to)) => from == to || images.ready().same_picture(from, to),
            _ => false,
        } {
            for url in [from, to].into_iter().flatten() {
                images.visible(url);
            }
            images.display(from, cover_area, ImageRenderPhase::Stable);
        } else {
            for url in [from, to].into_iter().flatten() {
                images.display(Some(url), cover_area, ImageRenderPhase::Resizing);
                images.prewarm(url, cover_area);
            }
        }
    }
}

/// 渲染右栏，端点直接画单态，途中按同一视图进度合成详情与封面。
///
/// # Params:
///   - `cover_in_flight`: 页面封面飞行层已接管时置真，只画详情文本
pub(crate) fn draw(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &NowPlayingView<'_>,
    theme: &Theme,
    cover_in_flight: bool,
) {
    let view = &state.input.switch;
    if view.at_min() {
        draw_view(frame, area, state, theme, View::Playlists, cover_in_flight);
    } else if view.at_max() {
        draw_view(frame, area, state, theme, View::Library, cover_in_flight);
    } else {
        // 两端只画文字，封面在合成后统一绘制，避免离屏帧持有终端图片。
        let from = transition::capture(frame, area, |frame| {
            draw_view(frame, area, state, theme, View::Playlists, true);
        });
        let to = transition::capture(frame, area, |frame| {
            draw_view(frame, area, state, theme, View::Library, true);
        });
        transition::panel(frame, Some(&from), Some(&to), area, view.raw(), theme);
        if !cover_in_flight {
            cover_transition::draw(frame, area, state, theme, view.eased_in_out());
        }
    }
}

/// 按显式视图绘制一端，切换目标不会把两端都变成同一种详情。
fn draw_view(
    frame: &mut Frame<'_>,
    area: Rect,
    state: &NowPlayingView<'_>,
    theme: &Theme,
    view: View,
    cover_in_flight: bool,
) {
    match view {
        View::Playlists => match state.input.playlist {
            Some(p) => playlist::draw(frame, area, p, state, theme, cover_in_flight),
            None => paint_empty(frame, area, theme),
        },
        View::Library => match state.input.track {
            Some(sv) => {
                let current_id = state.input.playing;
                track::draw(frame, area, sv, current_id, state, theme, cover_in_flight);
            }
            None => paint_empty(frame, area, theme),
        },
    }
}

/// 没有选中歌单或曲目时，保留 selected 标题和边框。
fn paint_empty(frame: &mut Frame<'_>, area: Rect, theme: &Theme) {
    let block = Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.surface1))
        .title(Line::from(" selected ").style(Style::new().fg(theme.subtext)));
    frame.render_widget(block, area);
}

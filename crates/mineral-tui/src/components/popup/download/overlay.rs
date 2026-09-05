//! downloads docked overlay.

use crossterm::event::KeyEvent;
use mineral_protocol::{DownloadOrigin, DownloadStatus, SongDownloadView};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Row, Table};

use crate::components::layout::shared::marquee::{
    MarqueeCtx, RowMarquee, resolve_column_widths, row_marquee,
};
use crate::components::layout::shared::scroll_table::render_scroll_table;
use crate::components::layout::shared::text::display_width;
use crate::components::popup::component::{
    Chrome, Overlay, OverlayAction, OverlayResponse, base_block,
};
use crate::render::theme::Theme;
use crate::runtime::action::Action;
use crate::runtime::marquee::Slot;
use crate::runtime::scroll::list::{ScrollList, ScrollMotion};
use crate::runtime::state::AppState;

/// Song title column, following the status icon.
const TITLE_COL: usize = 1;

/// Docked flat Song download list with a UI-local cursor.
pub(crate) struct DownloadOverlay {
    /// Cursor and smooth viewport state.
    list: ScrollList,
}

/// TUI presentation adapter for one protocol download row.
struct DownloadRow<'a> {
    /// Backend snapshot rendered by this row.
    download: &'a SongDownloadView,
}

impl DownloadRow<'_> {
    /// Builds the complete table row for the current download state.
    fn render(&self, theme: &Theme, marquee: Option<RowMarquee<'_>>) -> Row<'static> {
        let (icon, color) = self.status_icon(theme);
        Row::new([
            Line::from(Span::styled(icon, Style::new().fg(color))),
            self.song_title(marquee),
            Line::from(Span::styled(
                self.total_size(),
                Style::new().fg(theme.subtext),
            )),
            Line::from(Span::styled(self.metric(), Style::new().fg(theme.subtext))),
        ])
    }

    /// Scrolls only the selected title; each download entry has its own display identity.
    fn song_title(&self, marquee: Option<RowMarquee<'_>>) -> Line<'static> {
        let spans = vec![Span::raw(self.download.song.name.clone())];
        match marquee {
            Some(m) => m
                .ctx
                .line(spans, m.slot, self.download.id.as_str(), m.title_w),
            None => Line::from(spans),
        }
    }

    /// Returns the status icon and its semantic color.
    fn status_icon(&self, theme: &Theme) -> (&'static str, ratatui::style::Color) {
        match self.download.status {
            DownloadStatus::Queued => ("·", theme.overlay),
            DownloadStatus::Resolving
            | DownloadStatus::Downloading
            | DownloadStatus::Finalizing => ("↓", theme.accent),
            DownloadStatus::Stopping | DownloadStatus::Stopped => ("■", theme.yellow),
            DownloadStatus::Downloaded | DownloadStatus::AlreadyPresent => ("✓", theme.green),
            DownloadStatus::SkippedByHook => ("⊘", theme.yellow),
            DownloadStatus::Failed => ("✗", theme.red),
        }
    }

    /// Returns the total encoded size when the provider declared it.
    fn total_size(&self) -> String {
        self.download
            .bytes_total
            .map_or_else(String::new, fmt_bytes)
    }

    /// Returns progress, speed, or failure text when meaningful for the current state.
    fn metric(&self) -> String {
        match self.download.status {
            DownloadStatus::Downloading => match self.download.bytes_total {
                Some(total) if total > 0 => {
                    let percent = self
                        .download
                        .bytes_done
                        .saturating_mul(100)
                        .checked_div(total)
                        .unwrap_or_default()
                        .min(100);
                    format!("{percent}% {}", fmt_speed(self.download.speed_bps))
                }
                Some(_) | None => {
                    format!("{} unknown", fmt_bytes(self.download.bytes_done))
                }
            },
            DownloadStatus::Failed => self
                .download
                .failure
                .as_deref()
                .unwrap_or("Provider error")
                .to_owned(),
            DownloadStatus::Queued
            | DownloadStatus::Resolving
            | DownloadStatus::Finalizing
            | DownloadStatus::Stopping
            | DownloadStatus::Stopped
            | DownloadStatus::Downloaded
            | DownloadStatus::AlreadyPresent
            | DownloadStatus::SkippedByHook => String::new(),
        }
    }
}

impl DownloadOverlay {
    /// Creates the overlay at the first visible row.
    pub(crate) fn new() -> Self {
        Self {
            list: ScrollList::new(),
        }
    }

    /// Clamps the cursor after a snapshot changes length.
    pub(crate) fn clamp(&mut self, len: usize) {
        self.list.clamp(len);
    }

    /// Returns the selected row from the current snapshot.
    fn selected<'a>(&self, ctx: &'a AppState) -> Option<&'a SongDownloadView> {
        ctx.downloads.get(self.list.sel())
    }

    /// Sizes metadata columns to their content so empty metrics leave room for song titles.
    fn column_constraints(ctx: &AppState) -> [Constraint; 4] {
        let mut size_width = 0;
        let mut metric_width = 0;
        for download in &ctx.downloads {
            let row = DownloadRow { download };
            size_width = size_width.max(display_width(&row.total_size()));
            metric_width = metric_width.max(display_width(&row.metric()));
        }
        [
            Constraint::Length(2),
            Constraint::Fill(1),
            Constraint::Length(size_width.min(10)),
            Constraint::Length(metric_width.min(15)),
        ]
    }

    /// Builds the bottom-right border title for the aggregate summary.
    fn summary_footer(ctx: &AppState, theme: &Theme) -> Line<'static> {
        let summary = &ctx.downloads_summary;
        let label = format!(
            " ↓ {} songs  · {} queued  · {} ",
            summary.active,
            summary.queued,
            fmt_speed(summary.speed_bps)
        );
        Line::from(label)
            .right_aligned()
            .style(Style::new().fg(theme.subtext))
    }

    /// Builds the top-right border title for selected-row provenance and failure detail.
    fn detail_title(&self, ctx: &AppState, theme: &Theme) -> Option<Line<'static>> {
        let row = self.selected(ctx)?;
        let origin = match &row.origin {
            DownloadOrigin::Direct => "direct".to_owned(),
            DownloadOrigin::Playlist(playlist) => format!("from {}", playlist.name),
        };
        let base = format!(" {} · {origin}", row.quality.as_str());
        let label = row.failure.as_ref().map_or_else(
            || format!("{base} "),
            |failure| format!("{base} · {failure} "),
        );
        Some(
            Line::from(label)
                .right_aligned()
                .style(Style::new().fg(theme.overlay)),
        )
    }
}

impl Overlay for DownloadOverlay {
    fn chrome(&self) -> Chrome {
        Chrome {
            pct_w: 60,
            pct_h: 70,
            min_w: 40,
            min_h: 12,
            max_w: 96,
            max_h: 32,
            animated: true,
            dock: true,
            anchor: None,
            align: None,
        }
    }

    fn block(&self, ctx: &AppState, theme: &Theme, focused: bool) -> Block<'static> {
        let border = if focused {
            theme.accent
        } else {
            theme.surface1
        };
        let block = base_block(theme)
            .border_style(Style::new().fg(border))
            .title(Line::from(" downloads ").style(Style::new().fg(theme.subtext)))
            .title_bottom(Self::summary_footer(ctx, theme));
        match self.detail_title(ctx, theme) {
            Some(title) => block.title(title),
            None => block,
        }
    }

    fn render_content(&self, buf: &mut Buffer, inner: Rect, ctx: &AppState, theme: &Theme) {
        if inner.height == 0 {
            return;
        }
        let widths = Self::column_constraints(ctx);
        let title_width = resolve_column_widths(inner.width, &widths, /*selection_w*/ 2)
            .get(TITLE_COL)
            .copied()
            .unwrap_or_default();
        let marquee_ctx = MarqueeCtx::new(ctx, theme, theme.surface0);
        let rows = ctx
            .downloads
            .iter()
            .enumerate()
            .map(|(index, download)| {
                let marquee = row_marquee(
                    index == self.list.sel(),
                    &marquee_ctx,
                    Slot::DownloadSelected,
                    title_width,
                );
                DownloadRow { download }.render(theme, marquee)
            })
            .collect::<Vec<_>>();
        let table = Table::new(rows, widths)
            .row_highlight_style(
                Style::new()
                    .bg(theme.surface0)
                    .fg(theme.accent)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▌ ");
        render_scroll_table(
            buf,
            inner,
            table,
            &self.list,
            ctx.downloads.len(),
            usize::from(inner.height),
            ScrollMotion::Advancing {
                scrolloff: ctx.scrolloff(),
                glide_ticks: ctx.list_glide_ticks(),
            },
        );
    }

    fn on_key(&mut self, _key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        OverlayResponse::Pass
    }

    fn on_action(&mut self, action: Action, ctx: &AppState) -> Option<OverlayResponse> {
        self.list.clamp(ctx.downloads.len());
        match action {
            Action::MoveSelection(movement) => {
                self.list.move_by(movement, ctx.downloads.len());
                Some(OverlayResponse::Consumed)
            }
            Action::Scroll(step) => {
                let delta =
                    crate::runtime::scroll::viewport::step_delta(step, ctx.cfg.tui().behavior());
                self.list
                    .page(delta, ctx.downloads.len(), ctx.list_glide_ticks());
                Some(OverlayResponse::Consumed)
            }
            Action::DownloadSelection => Some(self.selected(ctx).map_or(
                OverlayResponse::Consumed,
                |download| {
                    if download.status.stoppable() {
                        OverlayResponse::Do(OverlayAction::StopDownload(download.id.clone()))
                    } else {
                        OverlayResponse::Consumed
                    }
                },
            )),
            Action::OpenDownloads | Action::BackOrClearSearch | Action::OpenQuitConfirm => {
                Some(OverlayResponse::Do(OverlayAction::CloseTop))
            }
            Action::ActivateSelection
            | Action::ReorderSelection(_)
            | Action::JumpToCurrent
            | Action::ToggleLoveSelection
            | Action::OpenActionMenu
            | Action::OpenCopyMenu
            | Action::EnterSearch
            | Action::DrillIntoSelection
            | Action::CycleDetailSection
            | Action::OpenQueue
            | Action::OpenSearchView
            | Action::ToggleFullscreen => Some(OverlayResponse::Consumed),
            Action::CycleLyricExtra
            | Action::TogglePlayPause
            | Action::CyclePlayMode
            | Action::NudgeVolume(_)
            | Action::SeekRelative(_)
            | Action::PrevOrRestart
            | Action::NextSong
            | Action::DismissNotice
            | Action::OpenHelp
            | Action::InvokeScript(_) => None,
        }
    }
}

/// Formats bytes with integer fixed-point units.
fn fmt_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    if bytes >= MB {
        let tenths = bytes.saturating_mul(10) / MB;
        format!("{}.{} MB", tenths / 10, tenths % 10)
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else {
        format!("{bytes} B")
    }
}

/// Formats aggregate speed with integer fixed-point units.
fn fmt_speed(bytes_per_second: u64) -> String {
    format!("{}/s", fmt_bytes(bytes_per_second))
}

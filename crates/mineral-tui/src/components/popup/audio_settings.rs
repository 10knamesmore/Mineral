//! Keyboard-only output selection using devices and stream state from the daemon.

use crossterm::event::{KeyCode, KeyEvent};
use mineral_audio::{OutputDevice, OutputTarget};
use mineral_client::operation::Outcome;
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Cell, Paragraph, Row, StatefulWidget, Table, TableState, Widget};

use super::component::{Chrome, Overlay, OverlayAction, OverlayResponse, base_block};
use crate::render::theme::Theme;
use crate::runtime::action::Action;
use crate::runtime::scroll::cursor::ListCursor;
use crate::runtime::state::AppState;

/// Displays output devices and tracks the cursor and pending switch.
pub(crate) struct AudioSettingsOverlay {
    /// Successful enumeration; absent while loading or after a query failure.
    devices: Option<Vec<OutputDevice>>,

    /// Device cursor; row zero is the explicit System default choice.
    cursor: ListCursor,

    /// Ratatui viewport offset retained across frames.
    viewport_offset: std::cell::Cell<usize>,

    /// Prevents repeated activation while a stream switch is in progress.
    switching: bool,

    /// Frontend-owned error text, never backend diagnostics.
    error: Option<&'static str>,
}

impl AudioSettingsOverlay {
    /// Creates a popup waiting for the output device list.
    pub(crate) fn new() -> Self {
        Self {
            devices: None,
            cursor: ListCursor::new(0),
            viewport_offset: std::cell::Cell::new(0),
            switching: false,
            error: None,
        }
    }

    /// Installs the enumerated devices and initially selects the confirmed route.
    pub(crate) fn apply_devices(&mut self, outcome: Outcome<Vec<OutputDevice>>, ctx: &AppState) {
        match outcome {
            Outcome::Applied(devices) | Outcome::Accepted(devices) => {
                let selected = ctx
                    .playback
                    .output
                    .as_ref()
                    .and_then(|output| match &output.target {
                        OutputTarget::SystemDefault => None,
                        OutputTarget::Device(id) => devices
                            .iter()
                            .position(|device| &device.id == id)
                            .map(|index| index + 1),
                    })
                    .unwrap_or(0);
                self.cursor.set(selected);
                self.devices = Some(devices);
                self.error = None;
            }
            Outcome::Failed { detail, .. } | Outcome::Unknown { detail } => {
                mineral_log::warn!(target: "tui", detail, "audio device enumeration failed");
                self.error = Some("Could not load output devices");
            }
        }
    }

    /// Releases the pending switch without changing any confirmed playback state.
    pub(crate) fn apply_selection(&mut self, outcome: &Outcome<()>) {
        self.switching = false;
        self.error = match outcome {
            Outcome::Applied(()) | Outcome::Accepted(()) => None,
            Outcome::Failed { detail, .. } => {
                mineral_log::warn!(target: "tui", detail, "audio output switch failed");
                Some("Could not switch output device")
            }
            Outcome::Unknown { detail } => {
                mineral_log::warn!(target: "tui", detail, "audio output switch result unavailable");
                Some("Output switch result unavailable")
            }
        };
    }

    /// Submits the highlighted route once; cursor movement alone never changes output.
    fn activate(&mut self) -> OverlayResponse {
        let Some(devices) = &self.devices else {
            return OverlayResponse::Consumed;
        };
        if self.switching {
            return OverlayResponse::Consumed;
        }
        let target = match self.cursor.sel().checked_sub(1) {
            None => OutputTarget::SystemDefault,
            Some(index) => {
                let Some(device) = devices.get(index) else {
                    return OverlayResponse::Consumed;
                };
                OutputTarget::Device(device.id.clone())
            }
        };
        self.switching = true;
        self.error = None;
        OverlayResponse::Do(OverlayAction::SelectAudioOutput(target))
    }

    /// Renders one row per device, keeping current output distinct from system default.
    fn render_devices(&self, buf: &mut Buffer, area: Rect, ctx: &AppState, theme: &Theme) {
        let Some(devices) = &self.devices else {
            if self.error.is_none() {
                Paragraph::new(" Loading devices…")
                    .style(Style::new().fg(theme.overlay))
                    .render(area, buf);
            }
            return;
        };
        let default_name = devices
            .iter()
            .find(|device| device.is_default)
            .map_or("—", |device| device.name.as_str());
        let mut rows = vec![Row::new(vec![
            Cell::from("System default"),
            Cell::from(Line::from(default_name).right_aligned())
                .style(Style::new().fg(theme.overlay)),
        ])];
        for device in devices {
            let active = ctx
                .playback
                .output
                .as_ref()
                .is_some_and(|output| output.device_id == device.id);
            let status = match (active, device.is_default) {
                (true, true) => "● active · default",
                (true, false) => "● active",
                (false, true) => "default",
                (false, false) => "",
            };
            rows.push(Row::new(vec![
                Cell::from(device.name.as_str()),
                Cell::from(Line::from(status).right_aligned()).style(Style::new().fg(if active {
                    theme.green
                } else {
                    theme.overlay
                })),
            ]));
        }
        let mut table = TableState::default()
            .with_offset(self.viewport_offset.get())
            .with_selected(Some(self.cursor.sel()));
        StatefulWidget::render(
            Table::new(rows, [Constraint::Fill(1), Constraint::Length(20)])
                .style(Style::new().fg(theme.text))
                .highlight_symbol("▌ ")
                .row_highlight_style(
                    Style::new()
                        .bg(theme.surface0)
                        .fg(theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
            area,
            buf,
            &mut table,
        );
        self.viewport_offset.set(table.offset());
    }
}

impl Overlay for AudioSettingsOverlay {
    fn chrome(&self) -> Chrome {
        let rows = self
            .devices
            .as_ref()
            .map_or(4, |devices| devices.len().saturating_add(1));
        let height = u16::try_from(rows.saturating_add(5))
            .unwrap_or(u16::MAX)
            .clamp(9, 16);
        Chrome {
            pct_w: 50,
            pct_h: 0,
            min_w: 52,
            min_h: height,
            max_w: 64,
            max_h: height,
            animated: true,
            dock: false,
            anchor: None,
            align: None,
        }
    }

    fn block(&self, _ctx: &AppState, theme: &Theme, focused: bool) -> Block<'static> {
        base_block(theme)
            .border_style(Style::new().fg(if focused {
                theme.accent
            } else {
                theme.surface1
            }))
            .title(Line::from(" Audio settings ").style(Style::new().fg(theme.subtext)))
    }

    fn render_content(&self, buf: &mut Buffer, inner: Rect, ctx: &AppState, theme: &Theme) {
        if inner.height < 3 || inner.width < 2 {
            return;
        }
        Paragraph::new(" Output device")
            .style(Style::new().fg(theme.subtext))
            .render(Rect::new(inner.x, inner.y, inner.width, 1), buf);
        self.render_devices(
            buf,
            Rect::new(
                inner.x,
                inner.y + 1,
                inner.width,
                inner.height.saturating_sub(3),
            ),
            ctx,
            theme,
        );
        if let Some(error) = self.error {
            Paragraph::new(format!(" {error}"))
                .style(Style::new().fg(theme.peach))
                .render(Rect::new(inner.x, inner.bottom() - 2, inner.width, 1), buf);
        }
        let format = ctx.playback.output.as_ref().map_or_else(
            || "—".to_owned(),
            |output| {
                format!(
                    "{} kHz · {} ch · {}",
                    f64::from(output.sample_rate_hz) / 1000.0,
                    output.channels,
                    output.sample_format
                )
            },
        );
        Paragraph::new(format!(" {format}"))
            .style(Style::new().fg(theme.subtext))
            .render(Rect::new(inner.x, inner.bottom() - 1, inner.width, 1), buf);
    }

    fn on_key(&mut self, key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        if key.code == KeyCode::Esc {
            OverlayResponse::Do(OverlayAction::CloseTop)
        } else {
            OverlayResponse::Pass
        }
    }

    fn on_action(&mut self, action: Action, _ctx: &AppState) -> Option<OverlayResponse> {
        match action {
            Action::OpenAudioSettings | Action::BackOrClearSearch | Action::OpenQuitConfirm => {
                Some(OverlayResponse::Do(OverlayAction::CloseTop))
            }
            Action::ActivateSelection | Action::DrillIntoSelection => Some(self.activate()),
            Action::MoveSelection(movement) => {
                if let Some(devices) = &self.devices {
                    self.cursor
                        .move_by(movement, devices.len().saturating_add(1));
                }
                Some(OverlayResponse::Consumed)
            }
            _ => None,
        }
    }
}

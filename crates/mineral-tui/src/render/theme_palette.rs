//! Select one palette for all terminal-dependent colors in the current configuration.

use palette::Srgb;
use ratatui::style::Color;

use crate::config::{
    AnsiSlot, ColorRef, ColorSource, ColorValue, HexColor, TerminalColor, TuiConfig,
};
use crate::terminal::{ColorReplies, TerminalPalette};

/// Reuse startup replies with the current configuration, including reloaded theme_defaults.
pub(crate) fn select_palette(config: &TuiConfig, replies: &ColorReplies) -> TerminalPalette {
    let defaults = config.theme_defaults();
    let fallback = TerminalPalette {
        foreground: rgb(*defaults.foreground()),
        background: rgb(*defaults.background()),
        ansi: defaults.ansi().map(rgb),
    };
    let missing = Requirements::of(config).missing(replies);
    if missing.is_empty() {
        replies.palette(fallback)
    } else {
        mineral_log::warn!(
            missing = ?missing,
            "terminal colors unavailable; using theme_defaults"
        );
        fallback
    }
}

#[derive(Default)]
struct Requirements {
    foreground: bool,

    background: bool,

    ansi: [bool; 16],
}

impl Requirements {
    fn of(config: &TuiConfig) -> Self {
        let mut required = Self::default();
        let theme = config.theme();
        let values = theme.colors();
        for value in values {
            required.color(value);
        }
        // Reset stays a terminal-default style, but background blending needs its RGB.
        if values
            .iter()
            .any(|value| matches!(value, ColorValue::Reset))
            || matches!(theme.background(), ColorRef::Value(ColorValue::Reset))
        {
            required.background = true;
        }
        if matches!(theme.text(), ColorValue::Reset) {
            required.foreground = true;
        }
        for reference in config
            .source_colors()
            .values()
            .chain([theme.background(), theme.search_hit().color()])
        {
            if let ColorRef::Value(value) = reference {
                required.color(value);
            }
        }
        required
    }

    fn color(&mut self, value: &ColorValue) {
        match value {
            ColorValue::Hex(_) | ColorValue::Reset => {}
            ColorValue::Ansi(slot) => self.ansi(*slot),
            ColorValue::Terminal(color) => self.terminal(*color),
            ColorValue::Mix(mix) => {
                self.source(mix.from());
                self.source(mix.to());
            }
        }
    }

    fn source(&mut self, source: &ColorSource) {
        match source {
            ColorSource::Hex(_) => {}
            ColorSource::Ansi(slot) => self.ansi(*slot),
            ColorSource::Terminal(color) => self.terminal(*color),
        }
    }

    fn ansi(&mut self, slot: AnsiSlot) {
        if let Some(needed) = self.ansi.get_mut(usize::from(slot.index())) {
            *needed = true;
        }
    }

    fn terminal(&mut self, color: TerminalColor) {
        match color {
            TerminalColor::Foreground => self.foreground = true,
            TerminalColor::Background => self.background = true,
        }
    }

    fn missing(&self, replies: &ColorReplies) -> Vec<String> {
        let mut missing = Vec::new();
        if self.foreground && replies.foreground.is_none() {
            missing.push("foreground (OSC 10)".to_owned());
        }
        if self.background && replies.background.is_none() {
            missing.push("background (OSC 11)".to_owned());
        }
        for (index, (needed, reply)) in self.ansi.iter().zip(replies.ansi).enumerate() {
            if *needed && reply.is_none() {
                missing.push(format!("ANSI {index} (OSC 4)"));
            }
        }
        missing
    }
}

pub(crate) fn color_of_value(value: &ColorValue, palette: &TerminalPalette) -> Color {
    match value {
        ColorValue::Hex(color) => render_color(rgb(*color)),
        ColorValue::Ansi(slot) => render_color(ansi(*slot, palette)),
        ColorValue::Terminal(color) => render_color(terminal(*color, palette)),
        ColorValue::Reset => Color::Reset,
        ColorValue::Mix(mix) => {
            let from = source(mix.from(), palette).into_format::<f64>();
            let to = source(mix.to(), palette).into_format::<f64>();
            let amount = *mix.amount();
            let mixed = Srgb::new(
                from.red + (to.red - from.red) * amount,
                from.green + (to.green - from.green) * amount,
                from.blue + (to.blue - from.blue) * amount,
            );
            render_color(mixed.into_format())
        }
    }
}

fn source(value: &ColorSource, palette: &TerminalPalette) -> Srgb<u8> {
    match value {
        ColorSource::Hex(color) => rgb(*color),
        ColorSource::Ansi(slot) => ansi(*slot, palette),
        ColorSource::Terminal(color) => terminal(*color, palette),
    }
}

fn terminal(color: TerminalColor, palette: &TerminalPalette) -> Srgb<u8> {
    match color {
        TerminalColor::Foreground => palette.foreground,
        TerminalColor::Background => palette.background,
    }
}

#[allow(clippy::indexing_slicing)] // AnsiSlot is validated to be within 0..=15.
fn ansi(slot: AnsiSlot, palette: &TerminalPalette) -> Srgb<u8> {
    palette.ansi[usize::from(slot.index())]
}

fn rgb(color: HexColor) -> Srgb<u8> {
    Srgb::new(color.r(), color.g(), color.b())
}

fn render_color(color: Srgb<u8>) -> Color {
    Color::Rgb(color.red, color.green, color.blue)
}

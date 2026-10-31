//! OSC 4/10/11 color replies, independent of configuration and rendering.

use std::fmt::Write as _;
use std::str;
use std::time::Duration;

use palette::Srgb;
use vte::{Parser, Perform};

use super::exchange::{Error, exchange};

/// RGB values used to resolve a theme after selecting terminal or bundled colors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TerminalPalette {
    pub(crate) foreground: Srgb<u8>,

    pub(crate) background: Srgb<u8>,

    pub(crate) ansi: [Srgb<u8>; 16],
}

/// Missing or malformed replies stay absent until theme requirements are known.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ColorReplies {
    pub(crate) foreground: Option<Srgb<u8>>,

    pub(crate) background: Option<Srgb<u8>>,

    pub(crate) ansi: [Option<Srgb<u8>>; 16],
}

impl ColorReplies {
    /// Query once in raw mode, before any input event reader is started.
    pub(crate) fn query(timeout: Duration) -> Result<Self, Error> {
        let mut command = String::from("\x1b]10;?\x1b\\\x1b]11;?\x1b\\");
        for index in 0..16 {
            let _ = write!(command, "\x1b]4;{index};?\x1b\\");
        }
        let response = exchange(&command, timeout, |bytes| Self::parse(bytes).complete())?;
        Ok(Self::parse(&response))
    }

    fn parse(bytes: &[u8]) -> Self {
        let mut replies = Self::default();
        Parser::new().advance(&mut replies, bytes);
        replies
    }

    fn complete(&self) -> bool {
        self.foreground.is_some()
            && self.background.is_some()
            && self.ansi.iter().all(Option::is_some)
    }

    /// Fill unused entries only after the caller has checked all required replies.
    pub(crate) fn palette(&self, bundled: TerminalPalette) -> TerminalPalette {
        let mut ansi = bundled.ansi;
        for (color, reply) in ansi.iter_mut().zip(self.ansi) {
            if let Some(reply) = reply {
                *color = reply;
            }
        }
        TerminalPalette {
            foreground: self.foreground.unwrap_or(bundled.foreground),
            background: self.background.unwrap_or(bundled.background),
            ansi,
        }
    }
}

impl Perform for ColorReplies {
    fn osc_dispatch(&mut self, parameters: &[&[u8]], _bell_terminated: bool) {
        match parameters {
            [b"10", color] => self.foreground = parse_rgb(color),
            [b"11", color] => self.background = parse_rgb(color),
            [b"4", entries @ ..] => {
                for [index, color] in entries.as_chunks::<2>().0 {
                    let index = str::from_utf8(index)
                        .ok()
                        .and_then(|value| value.parse::<usize>().ok());
                    if let Some(slot) = index.and_then(|index| self.ansi.get_mut(index)) {
                        *slot = parse_rgb(color);
                    }
                }
            }
            _ => {}
        }
    }
}

fn parse_rgb(bytes: &[u8]) -> Option<Srgb<u8>> {
    let value = str::from_utf8(bytes).ok()?.strip_prefix("rgb:")?;
    let mut components = value.split('/');
    let red = parse_channel(components.next()?)?;
    let green = parse_channel(components.next()?)?;
    let blue = parse_channel(components.next()?)?;
    components
        .next()
        .is_none()
        .then(|| Srgb::new(red, green, blue))
}

fn parse_channel(value: &str) -> Option<u8> {
    let bits = u32::try_from(value.len()).ok()?.checked_mul(4)?;
    if !(4..=16).contains(&bits) {
        return None;
    }
    let channel = u32::from_str_radix(value, 16).ok()?;
    let maximum = (1_u32 << bits) - 1;
    u8::try_from((channel * 255 + maximum / 2) / maximum).ok()
}

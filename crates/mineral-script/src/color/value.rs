//! Shared color values at the Lua boundary.
//!
//! `ColorValue` is the value exposed by the read-only `mineral` userdata
//! namespace and returned by `mineral.color.mix`. Hex and reset colors
//! serialize as strings; every other color serializes as a `[variant, payload]`
//! array so a JSON tree merge replaces it as one atomic value.

use std::fmt;

use derive_getters::Getters;
use serde::de::{Error, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// ANSI palette slot names in slot order `0..=15`.
pub(super) const ANSI_SLOT_NAMES: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "white",
    "bright_black",
    "bright_red",
    "bright_green",
    "bright_yellow",
    "bright_blue",
    "bright_magenta",
    "bright_cyan",
    "bright_white",
];

/// Selects a terminal color independently of where the resulting color is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalColor {
    Foreground,

    Background,
}

/// A validated terminal ANSI palette slot in `0..=15`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnsiSlot {
    pub(super) slot: u8,
}

impl AnsiSlot {
    /// Validated slot number (0..=15), resolved through the selected terminal palette.
    #[must_use]
    pub fn index(&self) -> u8 {
        self.slot
    }
}

impl Serialize for AnsiSlot {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u8(self.slot)
    }
}

impl<'de> Deserialize<'de> for AnsiSlot {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(AnsiSlotVisitor)
    }
}

struct AnsiSlotVisitor;

impl Visitor<'_> for AnsiSlotVisitor {
    type Value = AnsiSlot;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("an ANSI slot name (black through bright_white) or an index from 0 through 15")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        let slot = (0_u8..)
            .zip(ANSI_SLOT_NAMES)
            .find_map(|(slot, name)| (name == value).then_some(slot))
            .ok_or_else(|| {
                E::custom(format!(
                    "unknown ANSI slot name `{value}`; expected one of {ANSI_SLOT_NAMES:?}"
                ))
            })?;
        Ok(AnsiSlot { slot })
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: Error,
    {
        slot_from_index(value)
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: Error,
    {
        let Ok(index) = u64::try_from(value) else {
            return Err(E::custom(format!(
                "ANSI slot index must be from 0 through 15; got {value}"
            )));
        };
        slot_from_index(index)
    }
}

fn slot_from_index<E>(value: u64) -> Result<AnsiSlot, E>
where
    E: Error,
{
    match u8::try_from(value) {
        Ok(slot) if slot <= 15 => Ok(AnsiSlot { slot }),
        _ => Err(E::custom(format!(
            "ANSI slot index must be from 0 through 15; got {value}"
        ))),
    }
}

/// A `#rrggbb` color with validated RGB channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HexColor {
    r: u8,

    g: u8,

    b: u8,
}

impl HexColor {
    /// Red channel.
    #[must_use]
    pub fn r(&self) -> u8 {
        self.r
    }

    /// Green channel.
    #[must_use]
    pub fn g(&self) -> u8 {
        self.g
    }

    /// Blue channel.
    #[must_use]
    pub fn b(&self) -> u8 {
        self.b
    }
}

impl Serialize for HexColor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("#{:02x}{:02x}{:02x}", self.r, self.g, self.b))
    }
}

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        parse_hex(&raw)
    }
}

pub(super) fn parse_hex<E>(value: &str) -> Result<HexColor, E>
where
    E: Error,
{
    let hex = value
        .strip_prefix('#')
        .ok_or_else(|| E::custom(format!("hex color must start with `#`: `{value}`")))?;
    if hex.len() != 6 {
        return Err(E::custom(format!(
            "hex color must have exactly six hexadecimal digits after `#`: `{value}`"
        )));
    }
    let parse_pair = |index: usize| -> Result<u8, E> {
        let pair = hex
            .get(index..index + 2)
            .ok_or_else(|| E::custom(format!("invalid RGB channel in hex color `{value}`")))?;
        u8::from_str_radix(pair, 16).map_err(|error| {
            E::custom(format!(
                "hex color `{value}` contains non-hexadecimal characters: {error}"
            ))
        })
    };
    Ok(HexColor {
        r: parse_pair(0)?,
        g: parse_pair(2)?,
        b: parse_pair(4)?,
    })
}

/// A color that can be resolved to RGB for mixing.
///
/// Only hex, ANSI slots, and terminal foreground/background are valid sources;
/// reset and nested mixes are not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSource {
    Hex(HexColor),

    Ansi(AnsiSlot),

    Terminal(TerminalColor),
}

impl Serialize for ColorSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Hex(color) => color.serialize(serializer),
            Self::Ansi(slot) => ("ansi", slot).serialize(serializer),
            Self::Terminal(color) => ("terminal", color).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ColorSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ColorSourceVisitor)
    }
}

struct ColorSourceVisitor;

impl<'de> Visitor<'de> for ColorSourceVisitor {
    type Value = ColorSource;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a `#rrggbb` string or an [ansi|terminal, payload] color array")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        parse_hex(value).map(ColorSource::Hex)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let variant = next_variant(&mut sequence)?;
        let source = match variant.as_str() {
            "ansi" => ColorSource::Ansi(next_element(&mut sequence, "ansi slot")?),
            "terminal" => ColorSource::Terminal(next_element(&mut sequence, "terminal color")?),
            other => {
                return Err(Error::custom(format!(
                    "unknown color source variant `{other}`; expected ansi or terminal"
                )));
            }
        };
        reject_extra_elements(&mut sequence)?;
        Ok(source)
    }

    fn visit_map<A>(self, _map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        Err(Error::custom(
            "a mix source must be a #rrggbb string or a terminal color intent",
        ))
    }
}

/// Blends `from` toward `to`: amount 0 selects `from`, and amount 1 selects `to`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ColorMix {
    pub(super) from: ColorSource,

    pub(super) to: ColorSource,

    /// Finite fraction of the `to` color, from 0 through 1 inclusive.
    #[serde(deserialize_with = "deserialize_mix_amount")]
    pub(super) amount: f64,
}

fn deserialize_mix_amount<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let amount = f64::deserialize(deserializer)?;
    if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
        return Err(Error::custom(
            "mix amount must be a finite number from 0 through 1",
        ));
    }
    Ok(amount)
}

/// A palette color, including terminal colors and a blend of two color sources.
#[derive(Clone, Debug, PartialEq)]
pub enum ColorValue {
    Hex(HexColor),

    Ansi(AnsiSlot),

    Terminal(TerminalColor),

    Mix(Box<ColorMix>),

    Reset,
}

impl Serialize for ColorValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Hex(color) => color.serialize(serializer),
            Self::Reset => serializer.serialize_str("reset"),
            Self::Ansi(slot) => ("ansi", slot).serialize(serializer),
            Self::Terminal(color) => ("terminal", color).serialize(serializer),
            Self::Mix(mix) => ("mix", mix.as_ref()).serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for ColorValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ColorValueVisitor)
    }
}

struct ColorValueVisitor;

impl<'de> Visitor<'de> for ColorValueVisitor {
    type Value = ColorValue;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("`#rrggbb`, `reset`, or an [ansi|terminal|mix, payload] color array")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: Error,
    {
        if value == "reset" {
            return Ok(ColorValue::Reset);
        }
        parse_hex(value).map(ColorValue::Hex)
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let variant = next_variant(&mut sequence)?;
        let value = match variant.as_str() {
            "ansi" => ColorValue::Ansi(next_element(&mut sequence, "ansi slot")?),
            "terminal" => ColorValue::Terminal(next_element(&mut sequence, "terminal color")?),
            "mix" => ColorValue::Mix(Box::new(next_element(&mut sequence, "mix payload")?)),
            other => {
                return Err(Error::custom(format!(
                    "unknown color variant `{other}`; expected ansi, terminal, or mix"
                )));
            }
        };
        reject_extra_elements(&mut sequence)?;
        Ok(value)
    }

    fn visit_map<A>(self, _map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        Err(Error::custom(
            "a color must be a #rrggbb string, reset, or an intent from mineral.terminal or mineral.color.mix",
        ))
    }
}

fn next_variant<'de, A>(sequence: &mut A) -> Result<String, A::Error>
where
    A: SeqAccess<'de>,
{
    sequence
        .next_element()?
        .ok_or_else(|| Error::custom("color array must start with a variant name"))
}

fn next_element<'de, A, T>(sequence: &mut A, field: &str) -> Result<T, A::Error>
where
    A: SeqAccess<'de>,
    T: Deserialize<'de>,
{
    sequence
        .next_element()?
        .ok_or_else(|| Error::custom(format!("color array is missing its {field}")))
}

fn reject_extra_elements<'de, A>(sequence: &mut A) -> Result<(), A::Error>
where
    A: SeqAccess<'de>,
{
    if sequence.next_element::<IgnoredAny>()?.is_some() {
        return Err(Error::custom(
            "color array must contain exactly one variant name and one payload",
        ));
    }
    Ok(())
}

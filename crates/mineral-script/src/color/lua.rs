//! Read-only color constructors. They build intents without reading terminal colors.

use mlua::{AnyUserData, Lua, UserData, UserDataFields, UserDataMethods, Value};

use super::value::{ANSI_SLOT_NAMES, parse_hex};
use super::{AnsiSlot, ColorMix, ColorSource, ColorValue, TerminalColor};

impl UserData for ColorValue {}

struct AnsiColors;

impl UserData for AnsiColors {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        for (slot, name) in (0_u8..).zip(ANSI_SLOT_NAMES) {
            fields.add_field_method_get(name, move |lua, _| {
                lua.create_ser_userdata(ColorValue::Ansi(AnsiSlot { slot }))
            });
        }
    }
}

struct TerminalColors;

impl UserData for TerminalColors {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("foreground", |lua, _| {
            lua.create_ser_userdata(ColorValue::Terminal(TerminalColor::Foreground))
        });
        fields.add_field_method_get("background", |lua, _| {
            lua.create_ser_userdata(ColorValue::Terminal(TerminalColor::Background))
        });
        fields.add_field_method_get("ansi", |_, _| Ok(AnsiColors));
    }
}

struct ColorLibrary;

impl UserData for ColorLibrary {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_function(
            "mix",
            |lua, (from, to, amount): (Value, Value, f64)| -> mlua::Result<AnyUserData> {
                if !amount.is_finite() || !(0.0..=1.0).contains(&amount) {
                    return Err(mlua::Error::runtime(
                        "mix amount must be a finite number from 0 through 1",
                    ));
                }
                lua.create_ser_userdata(ColorValue::Mix(Box::new(ColorMix {
                    from: color_source_from_lua(&from)?,
                    to: color_source_from_lua(&to)?,
                    amount,
                })))
            },
        );
    }
}

struct MineralApi;

impl UserData for MineralApi {
    fn add_fields<F: UserDataFields<Self>>(fields: &mut F) {
        fields.add_field_method_get("terminal", |_, _| Ok(TerminalColors));
        fields.add_field_method_get("color", |_, _| Ok(ColorLibrary));
    }
}

/// Installs the global `mineral` color API before TUI configuration evaluation.
/// Namespace userdata exposes only getters and constructors; no setters.
pub fn install_color_api(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set("mineral", MineralApi)
}

fn color_source_from_lua(value: &Value) -> mlua::Result<ColorSource> {
    match value {
        Value::String(text) => parse_hex(&text.to_str()?).map(ColorSource::Hex),
        Value::UserData(userdata) if userdata.is::<ColorValue>() => {
            let color = userdata.borrow::<ColorValue>()?;
            match &*color {
                ColorValue::Hex(color) => Ok(ColorSource::Hex(*color)),
                ColorValue::Ansi(slot) => Ok(ColorSource::Ansi(*slot)),
                ColorValue::Terminal(color) => Ok(ColorSource::Terminal(*color)),
                ColorValue::Mix(_) | ColorValue::Reset => Err(mlua::Error::runtime(
                    "mix sources must be hex, ANSI, or terminal colors; reset and nested mixes are not allowed",
                )),
            }
        }
        other => Err(mlua::Error::runtime(format!(
            "mix source must be a #rrggbb string or a terminal color intent, got {}",
            other.type_name()
        ))),
    }
}

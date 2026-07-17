//! 主题色板:14 个 color token + 3 个语义角色映射(挂在 `TuiConfig` 下)。
//!
//! 对齐渲染层硬编码的 Catppuccin Mocha 色板。本模块只负责把用户颜色写法(固定
//! `#rrggbb` / 终端 ANSI 槽 / 终端默认)解析/校验成强类型,不依赖任何渲染框架;
//! 颜色到 client 接线处才落地。

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

/// 14 个合法 color token 名;`roles` 段的值必须取自此集合。
const TOKEN_NAMES: [&str; 14] = [
    "base", "mantle", "crust", "surface0", "surface1", "overlay", "subtext", "text", "accent",
    "accent_2", "red", "yellow", "green", "peach",
];

/// 主题色板:14 个 color token + 3 个语义角色映射。
///
/// 每个 token 是一个 [`ColorValue`](固定色 / ANSI 槽 / 终端默认);token 之间不能互相引用。
#[config_section]
pub struct ThemeConfig {
    /// 主背景。
    base: ColorValue,

    /// 次背景(嵌套面板 / 浮层底)。
    mantle: ColorValue,

    /// 第三背景(底部 transport / cmd 行)。
    crust: ColorValue,

    /// 行选中背景 / 进度条轨道。
    surface0: ColorValue,

    /// 未聚焦边框 / 分隔线。
    surface1: ColorValue,

    /// 暗淡文本 / 二级标签。
    overlay: ColorValue,

    /// 三级文本(metadata)。
    subtext: ColorValue,

    /// 主文本。
    text: ColorValue,

    /// 主强调色:选中 / 聚焦边框 / 当前播放。
    accent: ColorValue,

    /// 副强调色:进度条填充 / 频谱顶段。
    accent_2: ColorValue,

    /// 错误 / 删除 / love 标记。
    red: ColorValue,

    /// 暂停指示。
    yellow: ColorValue,

    /// 播放指示。
    green: ColorValue,

    /// 命令 / 搜索前缀。
    peach: ColorValue,

    /// 搜索命中字符的样式(色 + 叠加字体效果)。
    search_hit: SearchHitConfig,

    /// 封面驱动的动态主题(accent 随在播封面主色渐变)。
    dynamic: DynamicThemeConfig,
}

/// 封面驱动的动态主题:在播封面取色就绪后,`accent` / `accent_2` 从当前值
/// 渐变到封面派生色;无封面 / 取色失败渐变回本表的静态 token。
#[config_section]
pub struct DynamicThemeConfig {
    /// 是否启用(关闭即恒用静态 `accent` / `accent_2`)。
    enabled: bool,

    /// 切歌 / 封面就绪时 accent 渐变过去的时长,毫秒。
    fade_ms: u32,
}

/// 搜索命中字符的样式:在所在列的基础样式上叠加。
#[config_section]
pub struct SearchHitConfig {
    /// 高亮色:token 名(`"peach"` 等,随主题联动)或裸 `"#rrggbb"`。
    color: ColorRef,

    /// 叠加的字体效果(数组整体替换;空数组 = 仅变色)。
    modifiers: Vec<TextStyle>,
}

/// 一个**具体**颜色值:固定色 / 终端 ANSI 槽 / 终端默认。用于定义 14 个主题 token
/// (token 被下游引用,故此处不含指向别的 token 的间接引用,避免成环)。
///
/// Lua 写法:`"#rrggbb"` 简写,或单键结构化 table `{ hex = "#rrggbb" }` /
/// `{ ansi = "blue" }`(或编号 `{ ansi = 4 }`)/ `{ reset = true }`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorValue {
    /// 固定色。
    Hex(HexColor),

    /// 终端 16 个 ANSI 槽之一(实际 RGB 由终端配色决定,引用即"跟随终端")。
    Ansi(AnsiSlot),

    /// 终端默认前景 / 背景(不指定具体色)。
    Reset,
}

impl<'de> Deserialize<'de> for ColorValue {
    /// `#rrggbb` 字符串,或单键 table(`hex` / `ansi` / `reset`)。裸 token 名 / `token`
    /// 键 / 空 / 多键 / 未知键均报错(经 `serde_path_to_error` 带路径)。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ColorValueVisitor)
    }
}

/// [`ColorValue`] 的 serde 访问者:同时接受 `#rrggbb` 字符串与单键结构化 table。
struct ColorValueVisitor;

impl<'de> serde::de::Visitor<'de> for ColorValueVisitor {
    type Value = ColorValue;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("`#rrggbb` 字符串,或单键 table { hex / ansi / reset = ... }")
    }

    fn visit_str<E>(self, v: &str) -> Result<ColorValue, E>
    where
        E: serde::de::Error,
    {
        if v.starts_with('#') {
            parse_hex(v).map(ColorValue::Hex)
        } else {
            Err(serde::de::Error::custom(format!(
                "具体色须为 `#rrggbb` 或 table,不接受裸 token 名 `{v}`"
            )))
        }
    }

    fn visit_map<A>(self, mut map: A) -> Result<ColorValue, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let Some(key) = map.next_key::<String>()? else {
            return Err(serde::de::Error::custom(
                "颜色 table 不能为空,须含 hex / ansi / reset 之一",
            ));
        };
        let Some(value) = concrete_value_for_key(&key, &mut map)? else {
            return Err(serde::de::Error::custom(format!(
                "未知颜色键 `{key}`,具体色须取自 hex / ansi / reset"
            )));
        };
        reject_extra_keys(&mut map)?;
        Ok(value)
    }
}

/// 一处颜色引用:具体色值,或指向 14 个主题 token 之一(随主题联动)。search_hit /
/// 来源徽标用——它们**引用**调色板。
///
/// Lua 写法:具体色同 [`ColorValue`];token 引用用裸名(`"peach"`)或 `{ token = "peach" }`。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorRef {
    /// 具体色值(固定 / ANSI 槽 / 终端默认)。
    Value(ColorValue),

    /// 指向主题 token(如 `"peach"`),client 接线处按 token 名 resolve。
    Token(TokenName),
}

impl<'de> Deserialize<'de> for ColorRef {
    /// `#rrggbb` / 具体色 table → [`Self::Value`];裸名 / `{ token = .. }` → [`Self::Token`];
    /// 非法值原样冒出(经 `serde_path_to_error` 带路径)。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(ColorRefVisitor)
    }
}

/// [`ColorRef`] 的 serde 访问者:在 [`ColorValueVisitor`] 的具体色之外再认 token 引用。
struct ColorRefVisitor;

impl<'de> serde::de::Visitor<'de> for ColorRefVisitor {
    type Value = ColorRef;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("`#rrggbb` / token 名字符串,或单键 table { hex / token / ansi / reset = ... }")
    }

    fn visit_str<E>(self, v: &str) -> Result<ColorRef, E>
    where
        E: serde::de::Error,
    {
        if v.starts_with('#') {
            parse_hex(v).map(|h| ColorRef::Value(ColorValue::Hex(h)))
        } else {
            parse_token(v).map(ColorRef::Token)
        }
    }

    fn visit_map<A>(self, mut map: A) -> Result<ColorRef, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let Some(key) = map.next_key::<String>()? else {
            return Err(serde::de::Error::custom(
                "颜色 table 不能为空,须含 hex / token / ansi / reset 之一",
            ));
        };
        let parsed = if key == "token" {
            ColorRef::Token(map.next_value()?)
        } else if let Some(value) = concrete_value_for_key(&key, &mut map)? {
            ColorRef::Value(value)
        } else {
            return Err(serde::de::Error::custom(format!(
                "未知颜色键 `{key}`,须取自 hex / token / ansi / reset"
            )));
        };
        reject_extra_keys(&mut map)?;
        Ok(parsed)
    }
}

/// 读单键 table 里 `hex` / `ansi` / `reset` 三个具体色键之一;命中返 `Some` 并消费其值,
/// 其余键返 `None`(交调用方决定:ColorValue 报错,ColorRef 再试 `token`)。
fn concrete_value_for_key<'de, A>(key: &str, map: &mut A) -> Result<Option<ColorValue>, A::Error>
where
    A: serde::de::MapAccess<'de>,
{
    let value = match key {
        "hex" => ColorValue::Hex(map.next_value()?),
        "ansi" => ColorValue::Ansi(map.next_value()?),
        "reset" => {
            let on: bool = map.next_value()?;
            if !on {
                return Err(serde::de::Error::custom(
                    "`reset = false` 无意义:去掉该项或改用其他色",
                ));
            }
            ColorValue::Reset
        }
        _ => return Ok(None),
    };
    Ok(Some(value))
}

/// 确认单键 table 无多余键(读到第二个键即报错)。
fn reject_extra_keys<'de, A>(map: &mut A) -> Result<(), A::Error>
where
    A: serde::de::MapAccess<'de>,
{
    if let Some(extra) = map.next_key::<String>()? {
        return Err(serde::de::Error::custom(format!(
            "颜色 table 只能有一个键,多了 `{extra}`"
        )));
    }
    Ok(())
}

/// 借道 owned 反序列化把 `#rrggbb` 解析成 [`HexColor`](复用其格式校验;visitor 的 `&str`
/// 不保证 `'de` 生命周期,故转 owned 走 `StringDeserializer`)。
fn parse_hex<E>(v: &str) -> Result<HexColor, E>
where
    E: serde::de::Error,
{
    use serde::de::IntoDeserializer;
    HexColor::deserialize(v.to_owned().into_deserializer())
}

/// 同 [`parse_hex`],把裸名解析成经校验的 [`TokenName`]。
fn parse_token<E>(v: &str) -> Result<TokenName, E>
where
    E: serde::de::Error,
{
    use serde::de::IntoDeserializer;
    TokenName::deserialize(v.to_owned().into_deserializer())
}

/// 一个终端 ANSI 调色板槽(`0..=15`)。具名(`"blue"`)或编号(`4`)构造,反序列化校验范围;
/// 实际颜色由终端当前配色决定——引用它即"跟随终端"。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnsiSlot {
    /// 槽号,恒 ∈ `0..=15`(反序列化保证)。
    slot: u8,
}

impl AnsiSlot {
    /// 槽号(`0..=15`)。client 接线处据此映射到渲染框架的 ANSI 具名色。
    ///
    /// # Return:
    ///   `0..=15`
    pub fn index(&self) -> u8 {
        self.slot
    }
}

/// 16 个 ANSI 槽名,数组下标即槽号(`0..=15`)。
const ANSI_SLOT_NAMES: [&str; 16] = [
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

impl<'de> Deserialize<'de> for AnsiSlot {
    /// 具名(∈ [`ANSI_SLOT_NAMES`])或编号(`0..=15`);越界 / 未知名报错。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(AnsiSlotVisitor)
    }
}

/// [`AnsiSlot`] 的 serde 访问者:接受槽名字符串或整数槽号。
struct AnsiSlotVisitor;

impl serde::de::Visitor<'_> for AnsiSlotVisitor {
    type Value = AnsiSlot;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ANSI 槽名(black..bright_white)或槽号 0..=15")
    }

    fn visit_str<E>(self, v: &str) -> Result<AnsiSlot, E>
    where
        E: serde::de::Error,
    {
        match ANSI_SLOT_NAMES.iter().position(|n| *n == v) {
            Some(idx) => Ok(AnsiSlot {
                slot: u8::try_from(idx).unwrap_or(0),
            }),
            None => Err(serde::de::Error::custom(format!(
                "未知 ANSI 槽名 `{v}`,须取自 {ANSI_SLOT_NAMES:?}"
            ))),
        }
    }

    fn visit_u64<E>(self, v: u64) -> Result<AnsiSlot, E>
    where
        E: serde::de::Error,
    {
        slot_from_index(v)
    }

    fn visit_i64<E>(self, v: i64) -> Result<AnsiSlot, E>
    where
        E: serde::de::Error,
    {
        match u64::try_from(v) {
            Ok(n) => slot_from_index(n),
            Err(_) => Err(serde::de::Error::custom(format!(
                "ANSI 槽号须 0..=15,得 {v}"
            ))),
        }
    }
}

/// 把整数槽号校验进 `0..=15` 并造 [`AnsiSlot`],越界报错。
fn slot_from_index<E>(v: u64) -> Result<AnsiSlot, E>
where
    E: serde::de::Error,
{
    match u8::try_from(v) {
        Ok(slot) if slot <= 15 => Ok(AnsiSlot { slot }),
        _ => Err(serde::de::Error::custom(format!(
            "ANSI 槽号须 0..=15,得 {v}"
        ))),
    }
}

/// 可叠加的字体效果。终端实际渲染效果取决于终端模拟器支持(如部分终端无斜体)。
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    /// 加粗。
    Bold,

    /// 斜体。
    Italic,

    /// 下划线。
    Underline,

    /// 暗淡。
    Dim,

    /// 反色(前景背景互换)。
    Reversed,

    /// 删除线。
    CrossedOut,
}

/// 一个合法的 color token 名(∈ [`TOKEN_NAMES`])。反序列化时校验取值范围。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TokenName {
    /// 经校验的 token 名(保证 ∈ [`TOKEN_NAMES`])。
    name: String,
}

impl TokenName {
    /// 取 token 名(保证是 14 个合法 token 之一)。
    ///
    /// # Return:
    ///   token 名字符串切片
    pub fn as_str(&self) -> &str {
        &self.name
    }
}

impl<'de> Deserialize<'de> for TokenName {
    /// 解析 token 名:必须 ∈ [`TOKEN_NAMES`],否则报错(经 `serde_path_to_error` 带路径)。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        if TOKEN_NAMES.contains(&raw.as_str()) {
            Ok(Self { name: raw })
        } else {
            Err(serde::de::Error::custom(format!(
                "未知 token 名 `{raw}`,须取自 {TOKEN_NAMES:?}"
            )))
        }
    }
}

/// `#rrggbb` 十六进制颜色:三个具名 RGB 通道。反序列化即校验格式;以具名通道
/// 暴露(不用裸数组,client 接线处直接 `Color::Rgb(c.r(), c.g(), c.b())` 无需索引)。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HexColor {
    /// 红通道。
    r: u8,

    /// 绿通道。
    g: u8,

    /// 蓝通道。
    b: u8,
}

impl HexColor {
    /// 红通道。
    ///
    /// # Return:
    ///   `0..=255`
    pub fn r(&self) -> u8 {
        self.r
    }

    /// 绿通道。
    ///
    /// # Return:
    ///   `0..=255`
    pub fn g(&self) -> u8 {
        self.g
    }

    /// 蓝通道。
    ///
    /// # Return:
    ///   `0..=255`
    pub fn b(&self) -> u8 {
        self.b
    }
}

impl<'de> Deserialize<'de> for HexColor {
    /// 解析 `"#rrggbb"`(必须 `#` 前缀 + 6 位十六进制)→ 具名 RGB 通道;
    /// 非法格式返 `de::Error`(经 `serde_path_to_error` 带字段路径)。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let hex = raw
            .strip_prefix('#')
            .ok_or_else(|| serde::de::Error::custom(format!("颜色须以 `#` 开头:`{raw}`")))?;
        if hex.len() != 6 {
            return Err(serde::de::Error::custom(format!(
                "颜色须为 `#rrggbb` 共 6 位十六进制:`{raw}`"
            )));
        }
        let parse_pair = |idx: usize| -> Result<u8, D::Error> {
            let pair = hex
                .get(idx..idx + 2)
                .ok_or_else(|| serde::de::Error::custom(format!("颜色截取越界:`{raw}`")))?;
            u8::from_str_radix(pair, 16)
                .map_err(|e| serde::de::Error::custom(format!("颜色 `{raw}` 含非十六进制字符:{e}")))
        };
        Ok(Self {
            r: parse_pair(0)?,
            g: parse_pair(2)?,
            b: parse_pair(4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{HexColor, TokenName};

    /// theme.dynamic 段默认值:封面驱动 accent 默认开,过渡 3000ms。
    #[test]
    fn dynamic_defaults() -> color_eyre::Result<()> {
        let cfg = crate::Config::defaults()?;
        let d = cfg.tui().theme().dynamic();
        assert!(*d.enabled(), "封面驱动 accent 默认开");
        assert_eq!(*d.fade_ms(), 3000, "accent 过渡默认 3s");
        Ok(())
    }

    /// theme.dynamic 逐旋钮可覆盖:关掉 + 改时长都落到强类型。
    #[test]
    fn dynamic_override_takes_effect() -> color_eyre::Result<()> {
        let tree = crate::merge_tree(
            crate::default_tree()?,
            serde_json::json!({ "tui": { "theme": {
                "dynamic": { "enabled": false, "fade_ms": 500 },
            } } }),
        );
        let cfg =
            crate::from_tree(&tree).map_err(|w| color_eyre::eyre::eyre!("覆盖后应落型成功:{w}"))?;
        let d = cfg.tui().theme().dynamic();
        assert!(!*d.enabled());
        assert_eq!(*d.fade_ms(), 500);
        Ok(())
    }

    #[test]
    fn hex_color_parses_valid() -> color_eyre::Result<()> {
        let c: HexColor = serde_json::from_value(serde_json::json!("#1e1e2e"))?;
        assert_eq!((c.r(), c.g(), c.b()), (0x1e, 0x1e, 0x2e));
        Ok(())
    }

    #[test]
    fn hex_color_rejects_invalid() {
        assert!(
            serde_json::from_value::<HexColor>(serde_json::json!("1e1e2e")).is_err(),
            "缺 # 应报错"
        );
        assert!(
            serde_json::from_value::<HexColor>(serde_json::json!("#xyz")).is_err(),
            "非十六进制应报错"
        );
        assert!(
            serde_json::from_value::<HexColor>(serde_json::json!("#1e1e2")).is_err(),
            "位数不足应报错"
        );
    }

    #[test]
    fn token_name_validates_membership() -> color_eyre::Result<()> {
        let t: TokenName = serde_json::from_value(serde_json::json!("red"))?;
        assert_eq!(t.as_str(), "red");
        assert!(
            serde_json::from_value::<TokenName>(serde_json::json!("mauve")).is_err(),
            "非 token 名应报错"
        );
        Ok(())
    }

    /// ColorValue 具体色:`#` 前缀 string 与 `{ hex = "#.." }` table 都落 Hex(同格式,恒带 `#`)。
    #[test]
    fn color_value_hex_string_and_table() -> color_eyre::Result<()> {
        use super::ColorValue;

        let s: ColorValue = serde_json::from_value(serde_json::json!("#102030"))?;
        assert!(matches!(s, ColorValue::Hex(c) if (c.r(), c.g(), c.b()) == (0x10, 0x20, 0x30)));
        let t: ColorValue = serde_json::from_value(serde_json::json!({ "hex": "#102030" }))?;
        assert!(matches!(t, ColorValue::Hex(c) if (c.r(), c.g(), c.b()) == (0x10, 0x20, 0x30)));
        assert!(
            serde_json::from_value::<ColorValue>(serde_json::json!("#12345")).is_err(),
            "位数不足的 hex 应报错"
        );
        Ok(())
    }

    /// ColorValue 具体色:`{ ansi = ... }` 具名 / 编号都落 Ansi。
    #[test]
    fn color_value_ansi_table() -> color_eyre::Result<()> {
        use super::ColorValue;

        let named: ColorValue = serde_json::from_value(serde_json::json!({ "ansi": "blue" }))?;
        assert!(matches!(named, ColorValue::Ansi(s) if s.index() == 4));
        let numbered: ColorValue = serde_json::from_value(serde_json::json!({ "ansi": 8 }))?;
        assert!(matches!(numbered, ColorValue::Ansi(s) if s.index() == 8));
        Ok(())
    }

    /// ColorValue 具体色:`{ reset = true }` 落 Reset。
    #[test]
    fn color_value_reset_table() -> color_eyre::Result<()> {
        use super::ColorValue;

        let c: ColorValue = serde_json::from_value(serde_json::json!({ "reset": true }))?;
        assert!(matches!(c, ColorValue::Reset));
        Ok(())
    }

    /// ColorValue 拒绝 token(它定义调色板、不引用别人):裸名 / `{ token = .. }` 都报错;
    /// 空 / 多键 / 未知键 / `reset = false` 也报错。
    #[test]
    fn color_value_rejects_token_and_bad_table() {
        use super::ColorValue;

        for bad in [
            serde_json::json!("peach"),                           // 裸 token 名
            serde_json::json!({ "token": "peach" }),              // token table
            serde_json::json!({}),                                // 空 table
            serde_json::json!({ "ansi": "blue", "reset": true }), // 多键
            serde_json::json!({ "rgb": "102030" }),               // 未知键
            serde_json::json!({ "reset": false }),                // reset=false 无意义
        ] {
            assert!(
                serde_json::from_value::<ColorValue>(bad.clone()).is_err(),
                "应报错: {bad}"
            );
        }
    }

    /// ColorRef 在具体色之外还接受 token 引用:裸名 / `{ token = .. }` 落 Token,
    /// 其余具体写法委托给 [`ColorValue`] 落 `Value`。
    #[test]
    fn color_ref_accepts_token_and_delegates_to_value() -> color_eyre::Result<()> {
        use super::{ColorRef, ColorValue};

        let bare: ColorRef = serde_json::from_value(serde_json::json!("peach"))?;
        assert!(matches!(bare, ColorRef::Token(ref n) if n.as_str() == "peach"));
        let tbl: ColorRef = serde_json::from_value(serde_json::json!({ "token": "peach" }))?;
        assert!(matches!(tbl, ColorRef::Token(ref n) if n.as_str() == "peach"));

        let hex: ColorRef = serde_json::from_value(serde_json::json!("#102030"))?;
        assert!(
            matches!(hex, ColorRef::Value(ColorValue::Hex(c)) if (c.r(), c.g(), c.b()) == (0x10, 0x20, 0x30))
        );
        let ansi: ColorRef = serde_json::from_value(serde_json::json!({ "ansi": "blue" }))?;
        assert!(matches!(ansi, ColorRef::Value(ColorValue::Ansi(s)) if s.index() == 4));
        let reset: ColorRef = serde_json::from_value(serde_json::json!({ "reset": true }))?;
        assert!(matches!(reset, ColorRef::Value(ColorValue::Reset)));

        assert!(
            serde_json::from_value::<ColorRef>(serde_json::json!("mauve")).is_err(),
            "非 token 名应报错"
        );
        Ok(())
    }

    /// AnsiSlot 具名解析:16 个 ANSI 槽名各落到对应槽号(0..=15)。
    #[test]
    fn ansi_slot_named_parses_all_sixteen() -> color_eyre::Result<()> {
        use super::AnsiSlot;

        let cases = [
            ("black", 0),
            ("red", 1),
            ("green", 2),
            ("yellow", 3),
            ("blue", 4),
            ("magenta", 5),
            ("cyan", 6),
            ("white", 7),
            ("bright_black", 8),
            ("bright_red", 9),
            ("bright_green", 10),
            ("bright_yellow", 11),
            ("bright_blue", 12),
            ("bright_magenta", 13),
            ("bright_cyan", 14),
            ("bright_white", 15),
        ];
        for (name, idx) in cases {
            let s: AnsiSlot = serde_json::from_value(serde_json::json!(name))?;
            assert_eq!(s.index(), idx, "槽名 {name} 应落到 {idx}");
        }
        Ok(())
    }

    /// AnsiSlot 编号解析:0..=15 直接给整数。
    #[test]
    fn ansi_slot_numeric_parses() -> color_eyre::Result<()> {
        use super::AnsiSlot;

        for idx in [0_u8, 4, 15] {
            let s: AnsiSlot = serde_json::from_value(serde_json::json!(idx))?;
            assert_eq!(s.index(), idx);
        }
        Ok(())
    }

    /// AnsiSlot 越界编号 / 未知槽名报错。
    #[test]
    fn ansi_slot_rejects_out_of_range_and_bad_name() {
        use super::AnsiSlot;

        assert!(
            serde_json::from_value::<AnsiSlot>(serde_json::json!(16)).is_err(),
            "槽号 16 越界应报错"
        );
        assert!(
            serde_json::from_value::<AnsiSlot>(serde_json::json!("mauve")).is_err(),
            "未知槽名应报错"
        );
    }

    /// TextStyle 按 snake_case 字符串解析,未知效果名报错。
    #[test]
    fn text_style_parses_known_rejects_unknown() -> color_eyre::Result<()> {
        use super::TextStyle;

        let s: Vec<TextStyle> =
            serde_json::from_value(serde_json::json!(["bold", "italic", "crossed_out"]))?;
        assert_eq!(
            s,
            vec![TextStyle::Bold, TextStyle::Italic, TextStyle::CrossedOut]
        );
        assert!(
            serde_json::from_value::<TextStyle>(serde_json::json!("blink")).is_err(),
            "未知效果名应报错"
        );
        Ok(())
    }
}

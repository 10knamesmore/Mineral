//! 主题色板:14 个 color token + 3 个语义角色映射(挂在 `TuiConfig` 下)。
//!
//! 对齐渲染层硬编码的 Catppuccin Mocha 色板。本模块只负责把用户 `#rrggbb`
//! 字符串解析/校验成强类型,不依赖任何渲染框架;颜色到 client 接线处才落地。

use serde::Deserialize;

/// 14 个合法 color token 名;`roles` 段的值必须取自此集合。
const TOKEN_NAMES: [&str; 14] = [
    "base", "mantle", "crust", "surface0", "surface1", "overlay", "subtext", "text", "accent",
    "accent_2", "red", "yellow", "green", "peach",
];

/// 主题色板:14 个 color token + 3 个语义角色映射。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。颜色经 [`HexColor::rgb`] 取
/// RGB 三元组,client 接线处据此造各自渲染框架的颜色类型。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ThemeConfig {
    /// 主背景。
    base: HexColor,

    /// 次背景(嵌套面板)。
    mantle: HexColor,

    /// 第三背景(底部 transport / cmd 行)。
    crust: HexColor,

    /// 行选中 / 进度条 track。
    surface0: HexColor,

    /// 未聚焦边框 / 分隔线。
    surface1: HexColor,

    /// 暗淡文本 / 二级标签。
    overlay: HexColor,

    /// 三级文本(metadata)。
    subtext: HexColor,

    /// 主文本。
    text: HexColor,

    /// 主强调色:选中 / 聚焦边框 / 当前播放。
    accent: HexColor,

    /// 副强调色:进度条填充 / 频谱顶段。
    accent_2: HexColor,

    /// 错误 / 删除 / love 标记。
    red: HexColor,

    /// 暂停指示。
    yellow: HexColor,

    /// 播放指示。
    green: HexColor,

    /// 命令 / 搜索前缀。
    peach: HexColor,

    /// 语义角色 → token 名映射(accent / muted / faint)。
    roles: RolesConfig,

    /// 搜索命中字符的样式(色 + 叠加字体效果)。
    search_hit: SearchHitConfig,
}

/// 搜索命中字符的样式:在所在列的基础样式上叠加。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct SearchHitConfig {
    /// 高亮色:token 名(`"peach"` 等)或裸 `"#rrggbb"`。
    color: ColorRef,

    /// 叠加的字体效果(数组整体替换;空数组 = 仅变色)。
    modifiers: Vec<TextStyle>,
}

/// 一处颜色引用:既可指向 14 个主题 token 之一(随主题联动),也可直接给
/// `#rrggbb`(脱离 token 体系的固定色)。以 `#` 前缀区分两种写法。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ColorRef {
    /// 指向主题 token(如 `"peach"`),client 接线处按 token 名 resolve。
    Token(TokenName),

    /// 直接色值。
    Hex(HexColor),
}

impl<'de> Deserialize<'de> for ColorRef {
    /// `#` 开头按 [`HexColor`] 解析,否则按 [`TokenName`] 校验;两边的格式错误
    /// 原样冒出(经 `serde_path_to_error` 带字段路径)。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::IntoDeserializer;

        let raw = String::deserialize(deserializer)?;
        if raw.starts_with('#') {
            HexColor::deserialize(raw.into_deserializer()).map(Self::Hex)
        } else {
            TokenName::deserialize(raw.into_deserializer()).map(Self::Token)
        }
    }
}

/// 可叠加的字体效果。终端实际渲染效果取决于终端模拟器支持(如部分终端无斜体)。
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

/// 语义角色 → token 名映射。值是同表 14 个 token 之一的名字(如 `"red"`),
/// client 接线处解析为对应 token 的颜色。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取;值经 [`TokenName::as_str`] 取名。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct RolesConfig {
    /// 来源 accent 角色 → 哪个 token。
    accent: TokenName,

    /// 来源 muted 角色 → 哪个 token。
    muted: TokenName,

    /// 来源 faint 角色 → 哪个 token。
    faint: TokenName,
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

    /// ColorRef 双写法:`#` 前缀走 hex,否则按 token 名校验;两边的非法值都报错。
    #[test]
    fn color_ref_parses_token_and_hex() -> color_eyre::Result<()> {
        use super::ColorRef;

        let t: ColorRef = serde_json::from_value(serde_json::json!("peach"))?;
        assert!(matches!(t, ColorRef::Token(ref n) if n.as_str() == "peach"));
        let h: ColorRef = serde_json::from_value(serde_json::json!("#102030"))?;
        assert!(matches!(h, ColorRef::Hex(c) if (c.r(), c.g(), c.b()) == (0x10, 0x20, 0x30)));
        assert!(
            serde_json::from_value::<ColorRef>(serde_json::json!("mauve")).is_err(),
            "非 token 名应报错"
        );
        assert!(
            serde_json::from_value::<ColorRef>(serde_json::json!("#12345")).is_err(),
            "位数不足的 hex 应报错"
        );
        Ok(())
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

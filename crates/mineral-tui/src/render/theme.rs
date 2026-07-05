//! UI 主题色板:14 个 color token,生产路径由配置落地。
//!
//! 所有 widget 渲染都从 [`Theme`] 取色,避免散落的硬编码 RGB。来源徽标色是 per-source 配置
//! (`sources.<name>.color`),由 [`SourceColors`] 按 source 名解析(命中走配置色,未配置走中立
//! 兜底)——不用一个闭合调色枚举强塞进开放的来源集合。

use mineral_config::ColorRef;
use mineral_model::SourceKind;
use ratatui::style::{Color, Modifier, Style};

/// 一组完整的 UI 颜色 token。
///
/// 阶段 2 仅用到 `surface1 / subtext / accent / overlay` 等子集,其余字段在
/// 后续阶段(transport / spectrum / lyrics / overlay 等)启用,因此暂用
/// `#[allow(dead_code)]` 抑制 dead-code 警告。
#[allow(dead_code)] // reason: 后续阶段会逐步用到全部颜色 token
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// 主背景。
    pub base: Color,
    /// 次背景(嵌套面板)。
    pub mantle: Color,
    /// 第三背景(底部 transport / cmd 行)。
    pub crust: Color,
    /// 行选中 / 进度条 track。
    pub surface0: Color,
    /// 未聚焦边框 / 分隔线。
    pub surface1: Color,
    /// 暗淡文本 / 二级标签。
    pub overlay: Color,
    /// 三级文本(metadata)。
    pub subtext: Color,
    /// 主文本。
    pub text: Color,
    /// 主强调色:选中 / 聚焦边框 / 当前播放。
    pub accent: Color,
    /// 副强调色:进度条填充 / 频谱顶段。
    pub accent_2: Color,
    /// 错误 / 删除 / love 标记。
    pub red: Color,
    /// 暂停指示。
    pub yellow: Color,
    /// 播放指示。
    pub green: Color,
    /// 命令 / 搜索前缀。
    pub peach: Color,

    /// 搜索命中字符的高亮色(构造时按配置 `theme.search_hit.color` resolve:
    /// token 名随主题联动,`#rrggbb` 为固定色)。
    pub search_hit_color: Color,

    /// 搜索命中字符叠加的字体效果(`theme.search_hit.modifiers` 折叠)。
    pub search_hit_modifier: Modifier,
}

impl Theme {
    /// unavailable(来源标记「无可播资源」)行的整行降权样式:DIM 叠加,各列语义色不动。
    /// 只降视觉权重、**不禁选**——unavailable 是列表元数据口径,播放时取流失败自有
    /// 拦截脚本补救,禁选会堵死这条路。
    ///
    /// # Return:
    ///   叠加用样式(调用方 `Row::style` 应用)。
    pub fn unavailable_row(&self) -> Style {
        Style::new().add_modifier(Modifier::DIM)
    }

    /// 把配置里的 [`ColorRef`](token 名 / `#rrggbb`)解析成本主题下的具体色。
    ///
    /// token 名随主题联动(查 14 token 表),hex 为固定色。供 search_hit / 来源徽标色共用。
    ///
    /// # Params:
    ///   - `cr`: 配置里的颜色引用
    ///
    /// # Return:
    ///   落地颜色。
    pub fn resolve(&self, cr: &ColorRef) -> Color {
        match cr {
            ColorRef::Token(name) => self.token_by_name(name.as_str()),
            ColorRef::Hex(h) => Color::Rgb(h.r(), h.g(), h.b()),
        }
    }

    /// 从配置切片落地主题:14 token 各取一色 + search_hit 样式。
    ///
    /// # Params:
    ///   - `cfg`: 主题配置切片
    ///
    /// # Return:
    ///   落地后的 [`Theme`]。
    pub fn from_config(cfg: &mineral_config::ThemeConfig) -> Self {
        let c = |h: &mineral_config::HexColor| Color::Rgb(h.r(), h.g(), h.b());
        let mut t = Self {
            base: c(cfg.base()),
            mantle: c(cfg.mantle()),
            crust: c(cfg.crust()),
            surface0: c(cfg.surface0()),
            surface1: c(cfg.surface1()),
            overlay: c(cfg.overlay()),
            subtext: c(cfg.subtext()),
            text: c(cfg.text()),
            accent: c(cfg.accent()),
            accent_2: c(cfg.accent_2()),
            red: c(cfg.red()),
            yellow: c(cfg.yellow()),
            green: c(cfg.green()),
            peach: c(cfg.peach()),
            search_hit_color: Color::Reset,
            search_hit_modifier: Modifier::empty(),
        };
        t.search_hit_color = t.resolve(cfg.search_hit().color());
        t.search_hit_modifier = cfg
            .search_hit()
            .modifiers()
            .iter()
            .fold(Modifier::empty(), |acc, m| acc | modifier_of(*m));
        t
    }

    /// 按 token 名取对应字段色。
    /// 未知名按穷尽兜底回 `text`(不该发生)。
    fn token_by_name(&self, name: &str) -> Color {
        match name {
            "base" => self.base,
            "mantle" => self.mantle,
            "crust" => self.crust,
            "surface0" => self.surface0,
            "surface1" => self.surface1,
            "overlay" => self.overlay,
            "subtext" => self.subtext,
            "text" => self.text,
            "accent" => self.accent,
            "accent_2" => self.accent_2,
            "red" => self.red,
            "yellow" => self.yellow,
            "green" => self.green,
            "peach" => self.peach,
            _ => self.text,
        }
    }

    /// 默认主题:Catppuccin Mocha,accent = mauve / accent_2 = sapphire。
    ///
    /// **仅供测试对照与 `Default`**(spec Q5 裁决):生产构造一律走 [`Theme::from_config`],
    /// 与 `default.lua` 的 theme 段同值(由 `from_defaults_matches_mocha_mauve` 守卫)。
    pub const fn mocha_mauve() -> Self {
        Self {
            base: Color::Rgb(0x1e, 0x1e, 0x2e),
            mantle: Color::Rgb(0x18, 0x18, 0x25),
            crust: Color::Rgb(0x11, 0x11, 0x1b),
            surface0: Color::Rgb(0x31, 0x32, 0x44),
            surface1: Color::Rgb(0x45, 0x47, 0x5a),
            overlay: Color::Rgb(0x6c, 0x70, 0x86),
            subtext: Color::Rgb(0xa6, 0xad, 0xc8),
            text: Color::Rgb(0xcd, 0xd6, 0xf4),
            accent: Color::Rgb(0xcb, 0xa6, 0xf7),
            accent_2: Color::Rgb(0x74, 0xc7, 0xec),
            red: Color::Rgb(0xf3, 0x8b, 0xa8),
            yellow: Color::Rgb(0xf9, 0xe2, 0xaf),
            green: Color::Rgb(0xa6, 0xe3, 0xa1),
            peach: Color::Rgb(0xfa, 0xb3, 0x87),
            // search_hit 与 default.lua 的 `{ color = "peach",
            // modifiers = { "bold", "underline", "italic" } }` 对齐。
            search_hit_color: Color::Rgb(0xfa, 0xb3, 0x87),
            search_hit_modifier: Modifier::BOLD
                .union(Modifier::UNDERLINED)
                .union(Modifier::ITALIC),
        }
    }
}

/// 解析某来源的徽标色:从 `sources.<name>.color` 取该源的 [`ColorRef`],经主题落地成具体色;
/// 未配色的源(local / 未知插件)走中立兜底(`subtext`)。
///
/// 边渲染边解析(查一张两三项的小表 + resolve,开销可忽略),避免把颜色缓存塞进 `Theme`
/// (它是 `Copy` 的纯 token 板)。
///
/// # Params:
///   - `theme`: 已落地的主题(解析 token 名 / 兜底色用)
///   - `sources`: 音乐源段配置(各源的 `color`)
///   - `kind`: 目标来源
///
/// # Return:
///   徽标色。
pub fn resolve_source_color(
    theme: &Theme,
    sources: &mineral_config::SourcesConfig,
    kind: SourceKind,
) -> Color {
    sources
        .source_colors()
        .into_iter()
        .find(|(name, _)| *name == kind.name())
        .map(|(_, cr)| theme.resolve(cr))
        .unwrap_or(theme.subtext)
}

/// 配置层字体效果 → ratatui [`Modifier`] 的接线映射。
fn modifier_of(style: mineral_config::TextStyle) -> Modifier {
    match style {
        mineral_config::TextStyle::Bold => Modifier::BOLD,
        mineral_config::TextStyle::Italic => Modifier::ITALIC,
        mineral_config::TextStyle::Underline => Modifier::UNDERLINED,
        mineral_config::TextStyle::Dim => Modifier::DIM,
        mineral_config::TextStyle::Reversed => Modifier::REVERSED,
        mineral_config::TextStyle::CrossedOut => Modifier::CROSSED_OUT,
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::mocha_mauve()
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::SourceKind;

    use super::{Theme, resolve_source_color};

    /// 不写配置:from_config(defaults) 与 mocha_mauve 逐 token 一致(行为不变守卫;Theme 派生
    /// Debug,整体快照钉死)。
    #[test]
    fn from_defaults_matches_mocha_mauve() -> color_eyre::Result<()> {
        let cfg = mineral_config::Config::defaults()?;
        let t = Theme::from_config(cfg.tui().theme());
        let legacy = Theme::mocha_mauve();
        assert_eq!(
            format!("{t:?}"),
            format!("{legacy:?}"),
            "默认配置应逐字段等于 mocha_mauve"
        );
        crate::test_support::assert_snap_debug!(
            "Theme 默认值(from_config(defaults) = mocha_mauve)",
            t
        );
        Ok(())
    }

    /// 来源徽标色解析逻辑:已配置来源(bilibili 在 sources 段配了固定品牌色)解析成配置色、
    /// 不落中立兜底;未配置来源(LOCAL)落中立兜底(= subtext)。
    ///
    /// 只钉逻辑不钉具体色值——default.lua 里 bilibili 的实际品牌色由 `defaults_snapshot` 快照钉,
    /// 改色只需 review 快照,不必动本测试。
    #[test]
    fn source_colors_from_config_and_fallback() -> color_eyre::Result<()> {
        let cfg = mineral_config::Config::defaults()?;
        let theme = Theme::from_config(cfg.tui().theme());
        let sources = cfg.sources();
        assert_ne!(
            resolve_source_color(&theme, sources, SourceKind::BILIBILI),
            theme.subtext,
            "已配置来源(bilibili)解析成其配置色,不走中立兜底"
        );
        assert_eq!(
            resolve_source_color(&theme, sources, SourceKind::LOCAL),
            theme.subtext,
            "未配色来源(LOCAL)走中立兜底(subtext)"
        );
        Ok(())
    }

    /// 逐旋钮生效:search_hit 改成固定 hex 色 + 仅斜体,落地字段跟着变;
    /// token 写法(默认 peach)随主题联动由 defaults 守卫覆盖。
    #[test]
    fn search_hit_override_takes_effect() -> color_eyre::Result<()> {
        use ratatui::style::Modifier;

        let dir = tempfile::tempdir()?;
        let user = dir.path().join("config.lua");
        std::fs::write(
            &user,
            r##"return { tui = { theme = { search_hit = {
                color = "#102030", modifiers = { "italic" },
            } } } }"##,
        )?;
        let (cfg, warnings) = mineral_config::load(&user)?;
        assert!(warnings.is_empty(), "合法配置不应有 warning: {warnings:?}");
        let t = Theme::from_config(cfg.tui().theme());
        assert_eq!(
            t.search_hit_color,
            ratatui::style::Color::Rgb(0x10, 0x20, 0x30)
        );
        assert_eq!(
            t.search_hit_modifier,
            Modifier::ITALIC,
            "仅斜体,不带默认 bold"
        );
        Ok(())
    }

    /// 逐旋钮生效:改一个 token 色值,落地色与未改 token 互不影响。
    #[test]
    fn token_override_takes_effect() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let user = dir.path().join("config.lua");
        std::fs::write(
            &user,
            "return { tui = { theme = { accent = \"#102030\" } } }",
        )?;
        let (cfg, warnings) = mineral_config::load(&user)?;
        assert!(warnings.is_empty(), "合法配置不应有 warning: {warnings:?}");
        let t = Theme::from_config(cfg.tui().theme());
        assert_eq!(t.accent, ratatui::style::Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(t.base, Theme::mocha_mauve().base, "未改 token 不受影响");
        Ok(())
    }
}

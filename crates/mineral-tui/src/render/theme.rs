//! UI 主题色板:14 个 color token + 3 个语义角色,生产路径由配置落地。
//!
//! 所有 widget 渲染都从 [`Theme`] 取色,避免散落的硬编码 RGB。

use mineral_model::PaletteRole;
use ratatui::style::Color;

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

    /// 来源 Accent 角色落地色(构造时按配置 roles resolve)。
    role_accent: Color,

    /// 来源 Muted 角色落地色。
    role_muted: Color,

    /// 来源 Faint 角色落地色。
    role_faint: Color,
}

impl Theme {
    /// 把一个来源的语义调色角色([`PaletteRole`])解析成本主题下的具体颜色。
    ///
    /// 来源(含将来插件源)只声明角色,颜色在 [`Theme::from_config`] 构造时已按
    /// 配置 `theme.roles` resolve 进字段,这里只读——避免硬编码。
    pub const fn source_color(&self, role: PaletteRole) -> Color {
        match role {
            PaletteRole::Accent => self.role_accent,
            PaletteRole::Muted => self.role_muted,
            PaletteRole::Faint => self.role_faint,
        }
    }

    /// 从配置切片落地主题:14 token 各取一色,3 个角色按 token 名 resolve 成具体色。
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
            role_accent: Color::Reset,
            role_muted: Color::Reset,
            role_faint: Color::Reset,
        };
        t.role_accent = t.token_by_name(cfg.roles().accent().as_str());
        t.role_muted = t.token_by_name(cfg.roles().muted().as_str());
        t.role_faint = t.token_by_name(cfg.roles().faint().as_str());
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
            // roles 与 default.lua 的 `roles = { accent = "red", muted = "subtext",
            // faint = "overlay" }` 对齐。
            role_accent: Color::Rgb(0xf3, 0x8b, 0xa8),
            role_muted: Color::Rgb(0xa6, 0xad, 0xc8),
            role_faint: Color::Rgb(0x6c, 0x70, 0x86),
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::mocha_mauve()
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::PaletteRole;

    use super::Theme;

    /// 不写配置:from_config(defaults) 与 mocha_mauve 逐 token 一致、roles 落点不变
    /// (行为不变守卫;Theme 派生 Debug,整体快照钉死)。
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
            "Theme 默认值(from_config(defaults) = mocha_mauve,含 3 个 role 落地色)",
            t
        );
        Ok(())
    }

    /// 逐旋钮生效:用户配置改 roles.accent 指向 green,source_color(Accent) 跟着变。
    #[test]
    fn role_remap_takes_effect() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let user = dir.path().join("config.lua");
        std::fs::write(
            &user,
            "return { tui = { theme = { roles = { accent = \"green\" } } } }",
        )?;
        let (cfg, warnings) = mineral_config::load(&user)?;
        assert!(warnings.is_empty(), "合法配置不应有 warning: {warnings:?}");
        let t = Theme::from_config(cfg.tui().theme());
        assert_eq!(t.source_color(PaletteRole::Accent), t.green);
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

//! Catppuccin Mocha 主题色板。
//!
//! 所有 widget 渲染都从 [`Theme`] 取色,避免散落的硬编码 RGB。

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
}

impl Theme {
    /// 默认主题:Catppuccin Mocha,accent = mauve / accent_2 = sapphire。
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
        }
    }
}

impl Theme {
    /// 切 accent + accent_2 配色对。未识别的名称返回原主题不变。
    ///
    /// 支持的对(对应 Catppuccin Mocha 7 组):
    /// `mauve` (mauve+sapphire,默认)、`rosewater`(rosewater+pink)、
    /// `peach`(peach+yellow)、`green`(green+teal)、`sapphire`(sapphire+blue)、
    /// `blue`(blue+lavender)、`lavender`(lavender+mauve)。
    pub fn with_accent_pair(mut self, name: &str) -> Self {
        let pair = match name {
            "mauve" => Some((Color::Rgb(0xcb, 0xa6, 0xf7), Color::Rgb(0x74, 0xc7, 0xec))),
            "rosewater" => Some((Color::Rgb(0xf5, 0xe0, 0xdc), Color::Rgb(0xf5, 0xc2, 0xe7))),
            "peach" => Some((Color::Rgb(0xfa, 0xb3, 0x87), Color::Rgb(0xf9, 0xe2, 0xaf))),
            "green" => Some((Color::Rgb(0xa6, 0xe3, 0xa1), Color::Rgb(0x94, 0xe2, 0xd5))),
            "sapphire" => Some((Color::Rgb(0x74, 0xc7, 0xec), Color::Rgb(0x89, 0xb4, 0xfa))),
            "blue" => Some((Color::Rgb(0x89, 0xb4, 0xfa), Color::Rgb(0xb4, 0xbe, 0xfe))),
            "lavender" => Some((Color::Rgb(0xb4, 0xbe, 0xfe), Color::Rgb(0xcb, 0xa6, 0xf7))),
            _ => None,
        };
        if let Some((a, a2)) = pair {
            self.accent = a;
            self.accent_2 = a2;
        }
        self
    }
}

impl Default for Theme {
    fn default() -> Self {
        Self::mocha_mauve()
    }
}

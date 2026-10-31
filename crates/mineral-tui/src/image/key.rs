//! 定义图片引擎内部的图片身份与终端成品键。

use crate::config::CoverCellFit;
use mineral_model::MediaUrl;

/// 图片在终端中的目标像素尺寸。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PixelSize {
    /// 像素宽度。
    width: u32,

    /// 像素高度。
    height: u32,
}

impl PixelSize {
    /// 从像素宽高构造尺寸。
    pub(crate) const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    /// 从 cell 区域与单 cell 像素尺寸计算目标像素尺寸。
    ///
    /// # Params:
    ///   - `cells`: cell 宽高
    ///   - `cell_pixels`: 单个 cell 的像素宽高
    pub(crate) fn from_cells(cells: (u16, u16), cell_pixels: (u16, u16)) -> Self {
        Self {
            width: u32::from(cells.0).saturating_mul(u32::from(cell_pixels.0)),
            height: u32::from(cells.1).saturating_mul(u32::from(cell_pixels.1)),
        }
    }

    /// 返回像素宽度。
    pub(crate) const fn width(self) -> u32 {
        self.width
    }

    /// 返回像素高度。
    pub(crate) const fn height(self) -> u32 {
        self.height
    }
}

/// 低清预览的采样目标；主封面保留显示几何，行内封面保留独立的采样上限。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PreviewTarget {
    /// 主封面的完整 cell 画布，按终端物理像素比例采样并保留透明留白。
    Cover {
        /// 终端显示区域的物理像素宽高。
        pixels: PixelSize,

        /// 半块采样画布的字符列数与行数。
        cells: (u16, u16),

        /// 本次请求使用的图片边缘适配方式。
        cell_fit: CoverCellFit,
    },

    /// 行内封面的等比采样上限，不补边，最终尺寸由 Kitty placement 决定。
    Thumbnail { pixels: PixelSize },
}

impl PreviewTarget {
    pub(crate) fn cover(
        cells: (u16, u16),
        cell_pixels: (u16, u16),
        cell_fit: CoverCellFit,
    ) -> Self {
        Self::Cover {
            pixels: PixelSize::from_cells(cells, cell_pixels),
            cells,
            cell_fit,
        }
    }

    /// 行内封面等比采样到 32×32 以内，不补边、不保留完整解码图。
    pub(crate) const fn thumbnail() -> Self {
        Self::Thumbnail {
            pixels: PixelSize::new(32, 32),
        }
    }

    pub(crate) fn key(self, identity: ImageIdentity) -> TerminalImageKey {
        match self {
            Self::Cover {
                pixels,
                cells,
                cell_fit,
            } => TerminalImageKey::rasterized(identity, pixels, cells, cell_fit),
            Self::Thumbnail { pixels } => TerminalImageKey::thumbnail(identity, pixels),
        }
    }

    pub(crate) const fn pixels(self) -> PixelSize {
        match self {
            Self::Cover { pixels, .. } | Self::Thumbnail { pixels } => pixels,
        }
    }

    pub(crate) const fn cells(self) -> (u16, u16) {
        match self {
            Self::Cover { cells, .. } => cells,
            Self::Thumbnail { .. } => (32, 16),
        }
    }

    /// 配置热更时刷新后续预取的主封面策略，行内采样保持不变。
    pub(crate) fn with_cell_fit(self, cell_fit: CoverCellFit) -> Self {
        match self {
            Self::Cover { pixels, cells, .. } => Self::Cover {
                pixels,
                cells,
                cell_fit,
            },
            Self::Thumbnail { .. } => self,
        }
    }

    /// 先按 cell 和像素尺寸排序，相同尺寸的主封面先于行内采样预取。
    pub(crate) fn sort_key(self) -> (u16, u16, u32, u32, bool) {
        let cells = self.cells();
        let pixels = self.pixels();
        (
            cells.0,
            cells.1,
            pixels.width(),
            pixels.height(),
            matches!(self, Self::Thumbnail { .. }),
        )
    }
}

/// 引擎可显示的图片身份。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ImageIdentity {
    /// 由媒体 URL 标识的图片。
    Url(MediaUrl),
}

/// 一张终端图片成品的缓存身份。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TerminalImageKey {
    /// 保持原样的 Kitty 源图片；显示尺寸属于 placement。
    Source(ImageIdentity),

    /// 行内封面的低清像素或 Kitty 成品，由各自的缓存持有，独立于完整解码图。
    Thumbnail {
        /// 原始图片身份。
        identity: ImageIdentity,

        /// 等比采样的像素上限;Kitty 按行内 placement 决定最终显示尺寸。
        pixels: PixelSize,
    },

    /// 按物理像素、字符网格与格边适配生成的主封面。
    Rasterized {
        /// 原始图片身份。
        identity: ImageIdentity,

        /// 目标像素尺寸。
        pixels: PixelSize,

        /// 半块输出的字符列数与行数；物理像素相同的区域仍可能具有不同网格。
        cells: (u16, u16),

        /// 图片边缘的适配方式；热更后不能复用另一种方式的成品。
        cell_fit: CoverCellFit,
    },
}

impl TerminalImageKey {
    /// 构造只按图片身份缓存的源图片键。
    pub(crate) const fn source(identity: ImageIdentity) -> Self {
        Self::Source(identity)
    }

    /// 构造按低清采样上限缓存的 Kitty 缩略图键。
    pub(crate) const fn thumbnail(identity: ImageIdentity, pixels: PixelSize) -> Self {
        Self::Thumbnail { identity, pixels }
    }

    /// 构造包含完整采样几何的 rasterized 成品键。
    ///
    /// # Params:
    ///   - `identity`: 原始图片身份
    ///   - `pixels`: 目标像素尺寸
    ///   - `cells`: 半块输出的字符网格
    ///   - `cell_fit`: 本次生成使用的格边适配方式
    pub(crate) const fn rasterized(
        identity: ImageIdentity,
        pixels: PixelSize,
        cells: (u16, u16),
        cell_fit: CoverCellFit,
    ) -> Self {
        Self::Rasterized {
            identity,
            pixels,
            cells,
            cell_fit,
        }
    }

    /// 返回原始图片身份。
    pub(crate) const fn identity(&self) -> &ImageIdentity {
        match self {
            Self::Source(identity)
            | Self::Thumbnail { identity, .. }
            | Self::Rasterized { identity, .. } => identity,
        }
    }

    /// 返回本次编码的格边适配；源图片和行内缩略图始终保持完整比例。
    pub(crate) const fn cell_fit(&self) -> CoverCellFit {
        match self {
            Self::Rasterized { cell_fit, .. } => *cell_fit,
            Self::Source(_) | Self::Thumbnail { .. } => CoverCellFit::Contain,
        }
    }

    /// 判断成品键是否属于指定媒体 URL。
    #[cfg(test)]
    pub(crate) fn matches_url(&self, url: &MediaUrl) -> bool {
        matches!(self.identity(), ImageIdentity::Url(candidate) if candidate == url)
    }
}

//! 定义图片引擎内部的图片身份与终端成品键。

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

/// 引擎可显示的图片身份。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ImageIdentity {
    /// 由媒体 URL 标识的图片。
    Url(MediaUrl),
}

/// 一张终端图片成品的缓存身份。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum TerminalImageKey {
    /// Kitty 经 shared memory 发送的源图片；显示尺寸属于 placement。
    Source(ImageIdentity),

    /// Sixel、iTerm2 或 halfblocks 按目标像素生成的成品。
    Rasterized {
        /// 原始图片身份。
        identity: ImageIdentity,

        /// 目标像素尺寸。
        pixels: PixelSize,
    },
}

impl TerminalImageKey {
    /// 构造只按图片身份缓存的源图片键。
    pub(crate) const fn source(identity: ImageIdentity) -> Self {
        Self::Source(identity)
    }

    /// 构造按目标像素缓存的 rasterized 成品键。
    ///
    /// # Params:
    ///   - `identity`: 原始图片身份
    ///   - `pixels`: 目标像素尺寸
    pub(crate) const fn rasterized(identity: ImageIdentity, pixels: PixelSize) -> Self {
        Self::Rasterized { identity, pixels }
    }

    /// 返回原始图片身份。
    pub(crate) const fn identity(&self) -> &ImageIdentity {
        match self {
            Self::Source(identity) | Self::Rasterized { identity, .. } => identity,
        }
    }

    /// 返回 rasterized 成品的目标像素尺寸；源图片返回 `None`。
    pub(crate) const fn pixels(&self) -> Option<PixelSize> {
        match self {
            Self::Source(_) => None,
            Self::Rasterized { pixels, .. } => Some(*pixels),
        }
    }

    /// 判断成品键是否属于指定媒体 URL。
    #[cfg(test)]
    pub(crate) fn matches_url(&self, url: &MediaUrl) -> bool {
        matches!(self.identity(), ImageIdentity::Url(candidate) if candidate == url)
    }
}

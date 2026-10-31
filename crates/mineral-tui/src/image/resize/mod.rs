//! 封面的精确采样、等比适配与居中裁剪入口。

mod sampling;

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(super) use sampling::resize_exact;

pub(super) use sampling::{
    Error, PixelFit, fitted_pixels, resize_center_square, resize_to_fill, resize_transparent,
    scale_to_pixels, thumbnail, thumbnail_exact, thumbnail_to_fill,
};

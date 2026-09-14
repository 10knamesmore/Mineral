//! 封面的精确采样、等比适配与居中裁剪入口。

mod sampling;

#[cfg(test)]
mod tests;

pub(super) use sampling::{
    resize_exact, resize_to_fill, scale_to_pixels, thumbnail, thumbnail_exact,
};

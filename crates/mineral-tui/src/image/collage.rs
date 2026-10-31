//! 将已解码图片按一至四格合成方形封面，不选择歌单或调度加载。

use std::sync::Arc;

use image::imageops::replace;
use image::{DynamicImage, RgbImage};

use super::resize::{self, resize_to_fill};

/// 拼贴最多容纳四张图片，按两行两列排列。
pub(crate) const MAX_TILES: usize = 4;

/// 按输入顺序拼贴：一张全铺，两张左右分栏，三张左列整高，四张两行两列。
/// 输出边长取成员最小短边；每块居中裁剪，不足两像素时按两像素生成。
pub(crate) fn compose(images: &[Arc<DynamicImage>]) -> Result<Option<DynamicImage>, resize::Error> {
    let Some(size) = images
        .iter()
        .map(|image| image.width().min(image.height()))
        .min()
    else {
        return Ok(None);
    };
    let size = size.max(2);
    let half = size / 2;
    let rest = size - half;
    let mut canvas = RgbImage::new(size, size);
    let mut put = |image: &DynamicImage,
                   x: u32,
                   y: u32,
                   width: u32,
                   height: u32|
     -> Result<(), resize::Error> {
        let tile = resize_to_fill(image, width, height)?.into_rgb8();
        replace(&mut canvas, &tile, i64::from(x), i64::from(y));
        Ok(())
    };
    match images {
        [] => return Ok(None),
        [a] => put(a, 0, 0, size, size)?,
        [a, b] => {
            put(a, 0, 0, half, size)?;
            put(b, half, 0, rest, size)?;
        }
        [a, b, c] => {
            put(a, 0, 0, half, size)?;
            put(b, half, 0, rest, half)?;
            put(c, half, half, rest, rest)?;
        }
        [a, b, c, d, ..] => {
            put(a, 0, 0, half, half)?;
            put(b, half, 0, rest, half)?;
            put(c, 0, half, half, rest)?;
            put(d, half, half, rest, rest)?;
        }
    }
    Ok(Some(DynamicImage::ImageRgb8(canvas)))
}

//! 封面内容指纹：同一张图常挂在多个 URL 上(Netease 的 `param` 尺寸变体)，按 URL 相等
//! 认不出它们；这里把解码图降到固定灰度网格，用平均像素差相认。

use image::DynamicImage;

use super::resize::resize_exact;

/// 指纹网格边长(像素)。
const GRID: u32 = 16;

/// 判为同一张图允许的平均灰度差(0..=255)。同一张图换源尺寸 / 编码档位重采样通常只差
/// 个位数，不同封面远大于此；误判成同一张只会少一次肉眼难辨的交叉渐变。
const TOLERANCE: u32 = 4;

/// 一张封面图的内容指纹。
pub(crate) struct CoverFingerprint {
    /// 中心方阵降采样到 [`GRID`]×[`GRID`] 的灰度值(行优先)。
    gray: Box<[u8]>,
}

impl CoverFingerprint {
    /// 从解码图算指纹：取中心方阵，再降采样到固定网格。
    ///
    /// 在解码 worker(blocking 池)里调用，不进渲染路径。
    ///
    /// # Params:
    ///   - `image`: 已解码封面；非方图按中心方阵取样
    pub(crate) fn of(image: &DynamicImage) -> Self {
        let side = image.width().min(image.height());
        let square = image.crop_imm(
            (image.width() - side) / 2,
            (image.height() - side) / 2,
            side,
            side,
        );
        let sampled = resize_exact(&square, GRID, GRID).into_luma8();
        Self {
            gray: sampled.into_raw().into_boxed_slice(),
        }
    }

    /// 是否同一张图：采样点同样多且平均灰度差在 [`TOLERANCE`] 内。
    ///
    /// # Params:
    ///   - `other`: 另一张图(通常是另一个 URL)的指纹
    pub(crate) fn matches(&self, other: &Self) -> bool {
        if self.gray.len() != other.gray.len() {
            return false;
        }
        let mut total = 0_u64;
        for (left, right) in self.gray.iter().zip(&other.gray) {
            total = total.saturating_add(u64::from(left.abs_diff(*right)));
        }
        let samples = u64::try_from(self.gray.len()).unwrap_or(u64::MAX);
        total <= u64::from(TOLERANCE).saturating_mul(samples)
    }
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, Rgb, RgbImage};

    use super::{CoverFingerprint, GRID};

    /// 造一张 `side × side`、每个像素由 `pixel(x, y)` 决定的图。
    fn picture(side: u32, pixel: impl Fn(u32, u32) -> Rgb<u8>) -> DynamicImage {
        let mut image = RgbImage::new(side, side);
        for (x, y, cell) in image.enumerate_pixels_mut() {
            *cell = pixel(x, y);
        }
        DynamicImage::ImageRgb8(image)
    }

    /// 斜向渐变:按归一化坐标取值,同一图案在两种分辨率下一致。
    fn gradient(side: u32) -> DynamicImage {
        let span = (2 * side).saturating_sub(2).max(1);
        picture(side, |x, y| {
            let value = u8::try_from((x + y) * 255 / span).unwrap_or(u8::MAX);
            Rgb([value, 255 - value, value / 2])
        })
    }

    /// 竖条纹:与渐变在同分辨率下、在指纹网格上都差异明显。
    fn stripes(side: u32) -> DynamicImage {
        let column = (side / GRID).max(1);
        picture(side, |x, _| {
            let value = if (x / column).is_multiple_of(2) {
                0
            } else {
                255
            };
            Rgb([value, value, value])
        })
    }

    /// 同一张图换分辨率重采样后仍判为同一张。
    #[test]
    fn same_artwork_at_another_size_matches() {
        let small = CoverFingerprint::of(&gradient(64));
        let large = CoverFingerprint::of(&gradient(160));
        assert!(small.matches(&large), "同一张图换尺寸应相认");
        let same = CoverFingerprint::of(&gradient(64));
        assert!(small.matches(&same), "同一张图应相认");
    }

    /// 不同图案不算同一张。
    #[test]
    fn different_artwork_does_not_match() {
        let gradient = CoverFingerprint::of(&gradient(64));
        let stripes = CoverFingerprint::of(&stripes(64));
        let flat = CoverFingerprint::of(&picture(64, |_, _| Rgb([200, 0, 0])));
        assert!(!gradient.matches(&stripes), "条纹不是渐变");
        assert!(!gradient.matches(&flat), "纯色不是渐变");
        assert!(!stripes.matches(&flat), "纯色不是条纹");
    }

    /// 平色封面之间也要按亮度区分:黑与白不是同一张,差几档的同一色算同一张。
    #[test]
    fn flat_covers_compare_brightness() {
        let black = CoverFingerprint::of(&picture(32, |_, _| Rgb([0, 0, 0])));
        let white = CoverFingerprint::of(&picture(32, |_, _| Rgb([255, 255, 255])));
        let near_black = CoverFingerprint::of(&picture(48, |_, _| Rgb([3, 3, 3])));
        assert!(!black.matches(&white), "黑与白不是同一张");
        assert!(black.matches(&near_black), "同一档深色应相认");
    }
}

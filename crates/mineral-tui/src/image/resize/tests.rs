//! 采样后仍须保持图片格式、透明度以及终端使用的比例和裁剪规则。

use image::metadata::{CicpColorPrimaries, CicpTransferCharacteristics};
use image::{
    ColorType, DynamicImage, GenericImageView, ImageBuffer, Rgb, RgbImage, Rgba, RgbaImage,
};

use super::{resize_exact, resize_to_fill, scale_to_pixels, thumbnail, thumbnail_exact};
use crate::image::key::PixelSize;

/// 所有解码像素格式都能直接采样，保留位深与色彩标记，不强制转为 RGBA8。
#[test]
fn sampling_preserves_pixel_format_and_color_space() {
    for color in [
        ColorType::L8,
        ColorType::La8,
        ColorType::Rgb8,
        ColorType::Rgba8,
        ColorType::L16,
        ColorType::La16,
        ColorType::Rgb16,
        ColorType::Rgba16,
        ColorType::Rgb32F,
        ColorType::Rgba32F,
    ] {
        let mut source = DynamicImage::new(7, 5, color);
        source.set_rgb_primaries(CicpColorPrimaries::SmpteRp432);
        source.set_transfer_function(CicpTransferCharacteristics::Linear);
        for sampled in [resize_exact(&source, 3, 2), thumbnail_exact(&source, 3, 2)] {
            assert_eq!(sampled.dimensions(), (3, 2));
            assert_eq!(sampled.color(), color);
            assert_eq!(sampled.color_space(), source.color_space());
            assert!(sampled.as_bytes().iter().all(|byte| *byte == 0));
        }
    }
}

/// 缩小不能截成 8 位、丢掉 alpha，或在采样前改用 alpha 加权。
#[test]
fn sampling_preserves_high_bit_depth_and_straight_alpha() {
    let pixel = Rgba([1000_u16, 20000, 50000, 30000]);
    let source = DynamicImage::ImageRgba16(ImageBuffer::from_pixel(7, 5, pixel));
    for sampled in [resize_exact(&source, 3, 2), thumbnail_exact(&source, 3, 2)] {
        assert!(sampled.to_rgba16().pixels().all(|value| *value == pixel));
    }
    let source = DynamicImage::ImageRgba8(RgbaImage::from_fn(2, 1, |x, _| {
        if x == 0 {
            Rgba([255, 0, 0, 0])
        } else {
            Rgba([0, 0, 255, 255])
        }
    }));
    for sampled in [resize_exact(&source, 1, 1), thumbnail_exact(&source, 1, 1)] {
        let pixel = sampled.get_pixel(0, 0);
        for channel in [pixel.0[0], pixel.0[2], pixel.0[3]] {
            assert!((127..=128).contains(&channel));
        }
        assert_eq!(pixel.0[1], 0);
    }
}

/// 缩略图保持比例和四舍五入规则；终端画布仍左上对齐，空白处透明。
#[test]
fn fitting_preserves_aspect_ratio_and_transparent_padding() {
    for (source_size, bounds, fitted) in [
        ((9, 5), (6, 4), (6, 3)),
        ((5, 9), (4, 6), (3, 6)),
        ((3, 2), (5, 5), (5, 3)),
        ((1, 100), (32, 32), (1, 32)),
    ] {
        let pixel = Rgba([40, 80, 160, 255]);
        let source =
            DynamicImage::ImageRgba8(RgbaImage::from_pixel(source_size.0, source_size.1, pixel));
        assert_eq!(thumbnail(&source, bounds.0, bounds.1).dimensions(), fitted);
        let canvas = scale_to_pixels(&source, PixelSize::new(bounds.0, bounds.1));
        assert_eq!(canvas.dimensions(), bounds);
        for (x, y, value) in canvas.enumerate_pixels() {
            let expected = if x < fitted.0 && y < fitted.1 {
                pixel
            } else {
                Rgba([0, 0, 0, 0])
            };
            assert_eq!(*value, expected);
        }
    }
}

/// 拼图居中裁剪的内容与原有实现一致，允许采样整数舍入造成一个色阶的差异。
#[test]
fn collage_crops_keep_the_same_center_and_orientation() {
    for (size, target) in [((13, 7), (4, 5)), ((7, 13), (5, 4)), ((9, 5), (2, 2))] {
        let source = DynamicImage::ImageRgb8(RgbImage::from_fn(size.0, size.1, |x, y| {
            Rgb([
                if x < size.0 / 2 { 240 } else { 20 },
                if y < size.1 / 2 { 180 } else { 40 },
                100,
            ])
        }));
        let expected =
            source.resize_to_fill(target.0, target.1, image::imageops::FilterType::Triangle);
        let actual = resize_to_fill(&source, target.0, target.1);
        assert_eq!(actual.dimensions(), target);
        for (expected, actual) in expected.as_bytes().iter().zip(actual.as_bytes()) {
            assert!(
                expected.abs_diff(*actual) <= 1,
                "center crop or orientation changed"
            );
        }
    }
}

/// 不可见的零尺寸目标不生成像素；同尺寸采样不改变原有像素。
#[test]
fn empty_targets_and_identity_sampling_preserve_pixels() {
    let source = DynamicImage::ImageRgba8(RgbaImage::from_pixel(7, 5, Rgba([20, 40, 80, 120])));
    for (width, height) in [(0, 0), (0, 3), (3, 0), (7, 5)] {
        for output in [
            resize_exact(&source, width, height),
            thumbnail_exact(&source, width, height),
        ] {
            assert_eq!(output.dimensions(), (width, height));
            if width == 0 || height == 0 {
                assert!(output.as_bytes().is_empty());
            } else {
                assert_eq!(output.as_bytes(), source.as_bytes());
            }
        }
    }
}

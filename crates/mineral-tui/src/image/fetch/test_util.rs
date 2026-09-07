//! fetch 模块测试共享的小工具:临时目录与纯色图编码。

use std::io::Cursor;

use image::{DynamicImage, ImageFormat, RgbImage};

/// PID + 纳秒后缀的唯一临时目录。
pub(super) fn temp_dir() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "mineral-cover-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ))
}

/// 把指定尺寸的纯色 RGB 图编码成 PNG 字节,供 decode 测试喂入。
///
/// # Params:
///   - `w` / `h`: 目标宽高(像素)
///
/// # Return:
///   PNG 编码后的字节;编码失败返回 `Err`。
pub(super) fn png_bytes(w: u32, h: u32) -> color_eyre::Result<Vec<u8>> {
    let img = DynamicImage::ImageRgb8(RgbImage::new(w, h));
    let mut buf = Cursor::new(Vec::<u8>::new());
    img.write_to(&mut buf, ImageFormat::Png)
        .map_err(|e| color_eyre::eyre::eyre!("encode png: {e}"))?;
    Ok(buf.into_inner())
}

/// 同上但编码成 JPEG,用于验证扩展名嗅探认出 `jpg`。
///
/// # Params:
///   - `w` / `h`: 目标宽高(像素)
///
/// # Return:
///   JPEG 编码后的字节;编码失败返回 `Err`。
pub(super) fn jpeg_bytes(w: u32, h: u32) -> color_eyre::Result<Vec<u8>> {
    let img = DynamicImage::ImageRgb8(RgbImage::new(w, h));
    let mut buf = Cursor::new(Vec::<u8>::new());
    img.write_to(&mut buf, ImageFormat::Jpeg)
        .map_err(|e| color_eyre::eyre::eyre!("encode jpeg: {e}"))?;
    Ok(buf.into_inner())
}

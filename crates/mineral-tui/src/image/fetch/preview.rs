//! preview 管线:取压缩源 → 在 blocking 池解码 → 组装 halfblock 低清图。

use std::sync::Arc;

use image::DynamicImage;
use isahc::HttpClient;
use mineral_config::CoverConfig;
use mineral_persist::CacheIndex;

use crate::image::CoverFingerprint;
use crate::image::colors::extract_palette;
use crate::image::key::TerminalImageKey;
use crate::image::terminal::TerminalImage;
use crate::render::palette::CoverPalette;

use super::source::load_source;
use super::types::{CoverPreviewReady, CoverReady, PreviewRequest};

/// 非 JPEG 源一次解码顺路产出的显示图与色板。
pub(super) struct PreviewFull {
    /// 按配置尺寸解码的显示图(供 RAM LRU,消费方与 decode 落地一致)。
    image: Arc<DynamicImage>,

    /// 图片内容指纹(同一张图的不同 URL 靠它相认)。
    fingerprint: CoverFingerprint,

    /// 尽力而为的频谱色板。
    palette: Option<CoverPalette>,
}

/// preview worker 的一次解码产物:低清成品 + 非 JPEG 源顺路解码的显示图。
pub(super) struct PreviewResult {
    /// 可直接写入 ratatui buffer 的 halfblock preview。
    preview: TerminalImage,

    /// preview RGB 像素缓冲常驻字节数。
    resident: u64,

    /// 非 JPEG 源复用同次解码的显示图与色板;JPEG 的 preview 是低档位缩小解码,不带。
    full: Option<PreviewFull>,
}

/// 读取压缩源并生成目标尺寸的真实 halfblock preview。
///
/// # Params:
///   - `request`: 来源、URL 与 preview 几何
///   - `client`: Remote 图片 HTTP 客户端
///   - `cache`: 可用的磁盘缓存
///   - `cfg`: 解码与取色配置
///
/// # Return:
///   可直接缓存并渲染的 preview;取源或生成失败返回 `None`
pub(super) async fn fetch_preview(
    request: PreviewRequest,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
    cfg: &Arc<CoverConfig>,
) -> Option<(CoverPreviewReady, Option<CoverReady>)> {
    let PreviewRequest {
        source,
        url,
        key,
        cells,
    } = request;
    let bytes = load_source(source, &url, client, cache).await?;
    let pixels = key.pixels()?;
    let thumbnail = matches!(&key, TerminalImageKey::Thumbnail { .. });

    let result = {
        let cfg = Arc::clone(cfg);
        let preview = tokio::task::spawn_blocking(move || -> color_eyre::Result<PreviewResult> {
            let sample = |image| {
                if thumbnail {
                    TerminalImage::thumbnail_preview(&image, pixels)
                } else {
                    TerminalImage::halfblock_preview(image, pixels, cells)
                }
            };
            if matches!(image::guess_format(&bytes), Ok(image::ImageFormat::Jpeg)) {
                // JPEG 走低档位 IDCT 缩小解码,preview 很便宜;显示图仍需单独 decode,不带 full。
                let image = crate::image::decode::preview(&bytes, cells)?;
                let (preview, resident) = sample(image);
                return Ok(PreviewResult {
                    preview,
                    resident,
                    full: None,
                });
            }
            // 非 JPEG 源没有缩小解码能力,preview 本就要完整解码;一次解码同时产出
            // 显示图(供 RAM LRU)与 preview 源,复用同次解码,避免后续 decode 再解一遍。
            let image = crate::image::decode::display(&bytes, cfg.decode_pixels())?;
            let palette = extract_palette(&image, cfg.kmeans());
            let fingerprint = CoverFingerprint::of(&image);
            let (preview, resident) = sample(image.clone());
            Ok(PreviewResult {
                preview,
                resident,
                full: Some(PreviewFull {
                    image: Arc::new(image),
                    fingerprint,
                    palette,
                }),
            })
        })
        .await;
        match preview {
            Ok(Ok(result)) => Some(result),
            Ok(Err(error)) => {
                mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&error), "preview generation failed");
                None
            }
            Err(error) => {
                let error = color_eyre::Report::new(error);
                mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&error), "preview task join failed");
                None
            }
        }
    }?;

    let full = result.full.map(|full| CoverReady {
        url: url.clone(),
        image: full.image,
        fingerprint: full.fingerprint,
        palette: full.palette,
    });
    Some((
        CoverPreviewReady {
            url,
            key,
            image: result.preview,
            bytes: result.resident,
        },
        full,
    ))
}

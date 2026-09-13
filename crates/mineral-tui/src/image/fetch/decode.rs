//! decode 管线:取压缩源 → 在 blocking 池按配置尺寸解码显示图并提取色板。

use std::sync::Arc;

use image::DynamicImage;
use isahc::HttpClient;
use mineral_config::{CoverConfig, CoverDecodePixelsConfig};
use mineral_model::{MediaUrl, SourceKind};
use mineral_persist::CacheIndex;

use crate::image::CoverFingerprint;
use crate::image::colors::extract_palette;
use crate::render::palette::CoverPalette;

use super::source::load_source;

/// 解码产物:内存图 + 内容指纹 + 尽力而为的频谱色板。一次 `spawn_blocking` 内算完(都是 CPU 活儿)。
pub(super) struct DecodedCover {
    /// 解码后的内存图。
    pub(super) image: DynamicImage,

    /// 图片内容指纹(同一张图的不同 URL 靠它相认)。
    pub(super) fingerprint: CoverFingerprint,

    /// 从图提取的频谱色板(取色失败为 `None`)。
    pub(super) palette: Option<CoverPalette>,
}

/// 取一张封面并解码成内存图,优先磁盘缓存。
///
/// 命中:读盘原始压缩字节 → 解码。未命中(仅 Remote):下载 → 解码，并把下载字节写回
/// 缓存(`<source>/<hash>.<ext>`)。解码是 CPU 密集活儿，经
/// [`tokio::task::spawn_blocking`] 落 blocking 池，
/// 不占 runtime worker。Local:直接读盘解码,不进缓存。
///
/// # Params:
///   - `source`: 来源(决定缓存子目录)
///   - `url`: 封面来源 URL
///   - `client`: isahc 客户端(Remote 走它)
///   - `cache`: 磁盘缓存(可缺;`None` 直连不缓存)
///   - `cfg`: 封面段配置(kmeans)
///   - `pixels`: 提交请求时的解码目标
///
/// # Return:
///   解码后的图 + 色板;任一步失败返回 `None`。
pub(super) async fn fetch_and_decode(
    source: SourceKind,
    url: &MediaUrl,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
    cfg: &Arc<CoverConfig>,
    pixels: CoverDecodePixelsConfig,
) -> Option<DecodedCover> {
    let source_bytes = load_source(source, url, client, cache).await?;
    let cfg = Arc::clone(cfg);

    let decoded = tokio::task::spawn_blocking(move || -> color_eyre::Result<DecodedCover> {
        let image = crate::image::decode::display(&source_bytes, &pixels)?;
        mineral_log::debug!(target: "cover",
                    target_width = pixels.width().get(), target_height = pixels.height().get(),
                    decoded_width = image.width(), decoded_height = image.height(),
                    decoded_bytes = image.as_bytes().len(), "display cover decoded");
        let palette = extract_palette(&image, cfg.kmeans());
        let fingerprint = CoverFingerprint::of(&image);
        Ok(DecodedCover {
            image,
            fingerprint,
            palette,
        })
    })
    .await;

    match decoded {
        Ok(Ok(decoded)) => Some(decoded),
        Ok(Err(e)) => {
            mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&e), "decode failed");
            None
        }
        Err(e) => {
            mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&e), "decode task join failed");
            None
        }
    }
}

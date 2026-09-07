//! 压缩源字节读取层:本地读盘、Remote 下载与磁盘缓存读写。

use std::sync::Arc;

use color_eyre::eyre::{bail, eyre};
use isahc::{AsyncReadResponseExt, HttpClient};
use mineral_model::{MediaUrl, SourceKind};
use mineral_persist::CacheIndex;

/// 读取 Local 源或取得 Remote 压缩字节，Remote miss 时下载并尝试写入磁盘缓存。
///
/// # Params:
///   - `source`: 来源，决定 Remote 缓存子目录
///   - `url`: 图片源 URL
///   - `client`: Remote 图片 HTTP 客户端
///   - `cache`: 可用的磁盘缓存
///
/// # Return:
///   压缩源字节；读取或下载失败返回 `None`
pub(super) async fn load_source(
    source: SourceKind,
    url: &MediaUrl,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
) -> Option<Vec<u8>> {
    match url {
        MediaUrl::Remote(remote) => {
            let key = remote.as_str();
            if let Some(bytes) = cached_read(key, cache).await {
                return Some(bytes);
            }
            let started = std::time::Instant::now();
            let bytes = match download(client, key).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&error), "fetch failed");
                    return None;
                }
            };
            log_downloaded(key, started, bytes.len());
            if let Some(cache) = cache {
                let _ = store_source(cache, source, key, &bytes).await;
            }
            Some(bytes)
        }
        MediaUrl::Local(path) => {
            let bytes = match tokio::fs::read(path).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    let error = color_eyre::Report::new(error);
                    mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&error), "read file failed");
                    return None;
                }
            };
            Some(bytes)
        }
    }
}

/// 命中磁盘缓存时返回文件字节(Remote key);未命中 / 无缓存 / 读盘失败均 `None`(当 miss)。
///
/// # Params:
///   - `key`: 缓存键(= URL 串)
///   - `cache`: 磁盘缓存(可缺)
///
/// # Return:
///   命中且可读返回字节,否则 `None`。
async fn cached_read(key: &str, cache: Option<&Arc<CacheIndex>>) -> Option<Vec<u8>> {
    // get 只 stat,可直接同步调。
    let path = cache?.get(key)?;
    match tokio::fs::read(&path).await {
        Ok(bytes) => Some(bytes),
        Err(e) => {
            let e = color_eyre::Report::new(e);
            mineral_log::warn!(target: "cover", key = %key, error = mineral_log::chain(&e), "缓存文件读失败,回退网络");
            None
        }
    }
}

/// 打一条封面下载完成的 debug 日志。
///
/// 排查封面变慢问题的一手数据:`RUST_LOG=cover=debug` 打开。
///
/// # Params:
///   - `url`: 实际下载用的地址
///   - `started`: 本次请求起点
///   - `bytes`: 响应体大小
fn log_downloaded(url: &str, started: std::time::Instant, bytes: usize) {
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    mineral_log::debug!(target: "cover", %url, elapsed_ms, bytes, "封面下载完成");
}

/// 下载 Remote 封面的原始字节。
///
/// # Params:
///   - `client`: isahc 客户端
///   - `key`: 远端 URL
///
/// # Return:
///   原始字节;网络失败或非 2xx 状态返回 `Err`。
async fn download(client: &HttpClient, key: &str) -> color_eyre::Result<Vec<u8>> {
    let mut resp = client
        .get_async(key)
        .await
        .map_err(|e| eyre!("http: {e}"))?;
    if !resp.status().is_success() {
        bail!("http status {}", resp.status());
    }
    resp.bytes().await.map_err(|e| eyre!("read body: {e}"))
}

/// 把 Remote 压缩源数据写入磁盘缓存。
///
/// # Params:
///   - `cache`: 磁盘缓存
///   - `source`: 来源(子目录)
///   - `key`: 缓存键(= URL 串)
///   - `bytes`: 落盘字节
///
/// # Return:
///   写入成功时返回 `true`；失败已记录日志并返回 `false`
async fn store_source(
    cache: &Arc<CacheIndex>,
    source: SourceKind,
    key: &str,
    bytes: &[u8],
) -> bool {
    let file_name = cover_file_name(key, sniff_ext(bytes));
    match cache.put_bytes(key, bytes, source.name(), &file_name).await {
        Ok(()) => true,
        Err(error) => {
            mineral_log::warn!(target: "cover", key, error = mineral_log::chain(&error), "封面写缓存失败");
            false
        }
    }
}

/// 封面落盘文件名:`<key 哈希>.<ext>`。封面键是 URL,无可读标题,用哈希定一个稳定短名
/// (`CacheIndex` 仍以 URL 为索引键,文件名只需唯一)。
///
/// # Params:
///   - `key`: 缓存键(= URL 串)
///   - `ext`: 扩展名(不含点)
///
/// # Return:
///   形如 `1a2b3c4d5e6f7890.jpg`。
fn cover_file_name(key: &str, ext: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    format!("{:016x}.{ext}", h.finish())
}

/// 按魔数嗅探图片格式的扩展名,认不出退 `"img"`。不信 URL 后缀。
///
/// # Params:
///   - `bytes`: 图片字节
///
/// # Return:
///   扩展名(如 `jpg`/`png`/`webp`),无法识别返回 `img`。
fn sniff_ext(bytes: &[u8]) -> &'static str {
    match image::guess_format(bytes) {
        Ok(fmt) => fmt.extensions_str().first().copied().unwrap_or("img"),
        Err(_) => "img",
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use mineral_persist::ClientStore;

    use super::{cached_read, cover_file_name, download, sniff_ext};
    use crate::image::fetch::test_util::{jpeg_bytes, png_bytes, temp_dir};

    /// PNG 原始字节的缓存扩展名嗅探为 `png`。
    #[test]
    fn sniffs_png_extension() -> color_eyre::Result<()> {
        let png = png_bytes(/*w*/ 10, /*h*/ 10)?;
        assert_eq!(sniff_ext(&png), "png");
        Ok(())
    }

    /// JPEG 原始字节的缓存扩展名嗅探为 `jpg`。
    #[test]
    fn sniffs_jpeg_extension() -> color_eyre::Result<()> {
        let jpg = jpeg_bytes(/*w*/ 10, /*h*/ 10)?;
        assert_eq!(sniff_ext(&jpg), "jpg");
        Ok(())
    }

    /// 缓存命中时 `cached_read` 直读缓存文件返回字节(结构上不碰网络——它不收 client)。
    #[tokio::test]
    async fn cached_read_hits_disk() -> color_eyre::Result<()> {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir)?;
        let files = dir.join("files");
        let store = ClientStore::open(&dir.join("cover.db")).await?;
        let cache = Arc::new(store.cover_cache(files, 1024 * 1024).await?);
        let key = "http://192.0.2.1/cover.jpg";
        cache
            .put_bytes(
                key,
                b"cached-cover-bytes",
                /*subdir*/ "netease",
                &cover_file_name(key, "jpg"),
            )
            .await?;

        let bytes = cached_read(key, Some(&cache)).await;
        assert_eq!(
            bytes.as_deref(),
            Some(&b"cached-cover-bytes"[..]),
            "命中应直读缓存文件"
        );
        drop(cache);
        drop(std::fs::remove_dir_all(&dir));
        Ok(())
    }

    /// 非 2xx 响应按下载失败处理,不把错误页字节当图喂解码器。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn download_rejects_non_2xx() -> color_eyre::Result<()> {
        let url =
            mineral_test::mock::serve_once_status(/*status*/ 404, b"not found".to_vec()).await?;
        let client = isahc::HttpClient::new().map_err(|e| color_eyre::eyre::eyre!("isahc: {e}"))?;
        assert!(
            download(&client, url.as_str()).await.is_err(),
            "404 应判失败"
        );
        Ok(())
    }
}

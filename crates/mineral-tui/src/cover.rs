//! Client 端的封面图 fetcher。
//!
//! 4 个 tokio worker 共享一条 mpsc 队列:worker 上 async 抓字节,解码+resize 经
//! `spawn_blocking` 落到 blocking 线程池(CPU 密集,别占 runtime worker)。
//! 跟 mineral-task 的 lane 不同,本 fetcher **归 client 所有** —— 封面是装饰性
//! 资源,server 不该管。多 client 各持一个 fetcher,各 fetch 各 cache。
//!
//! 设计取舍:
//! - **不做 cancel**:用户切走时积压 fetch 仍跑完,结果进 cache 放着;下次显示直接命中。
//!   减一条复杂度,跟现在(server 端 cancel 后的 cache 命中行为)对齐。
//! - **不做内部 dedup**:dedup 由 caller(`prefetch::ensure_cover` 用 `state.cover_pending`
//!   集合)做。fetcher 单纯 FIFO worker pool。
//! - **错误静默**:fetch / decode 失败只打日志,不推 result,UI 自然显示 fallback。

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::eyre;
use image::DynamicImage;
use isahc::AsyncReadResponseExt;
use isahc::HttpClient;
use isahc::config::Configurable;
use mineral_model::{MediaUrl, SourceKind};
use mineral_persist::{CacheIndex, ClientStore};
use parking_lot::Mutex;
use tokio::sync::mpsc;

/// 单一 worker 池的并发度。封面图都是几十 KB,4 路够覆盖快速翻 selection。
const WORKERS: usize = 4;

/// HTTP 客户端 timeout。封面比 audio 流小得多,30s 足够慢网兜底。
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// 封面磁盘缓存容量上限(字节)。
const COVER_CACHE_CAPACITY: u64 = 1024 * 1024 * 1024;

/// 解码后 resize 到此最大边(像素),保持比例。
///
/// 终端 cell 典型 8×16 px,cover 面板大概 30 cols × 15 rows ≈ 240×240 px;
/// 384 远超显示需求,留点余量给高 DPI / 大字号。原图常常 1024×1024(网易裸 URL),
/// resize 到 384 内存降 ~7x,RGBA 256KB/张 vs 1.5MB+。视觉上完全无损。
const COVER_MAX_DIM: u32 = 384;

/// Resized 模式重编码 JPEG 的质量(0–100)。缩略图用,85 视觉无损且够小。
const COVER_JPEG_QUALITY: u8 = 85;

/// 封面磁盘缓存的存储形态。配置系统落地前由 [`COVER_STORAGE`] 选定,两种都实现。
#[derive(Clone, Copy)]
enum CoverStorageMode {
    /// 原始下载字节:无损原图,扩展名按字节嗅探(jpg/png/webp)。
    Raw,

    /// `decode_resize` 后重编码 JPEG:体积小,但锁定 ≤[`COVER_MAX_DIM`]。
    ///
    /// 当前 [`COVER_STORAGE`] 选 `Raw` 故非 test 构建不构造它(由 `bytes_for_cache` 测试覆盖);
    /// 配置系统接入后会被选用,届时移除本 allow。
    #[allow(dead_code)]
    Resized,
}

/// 当前封面磁盘存储模式。配置系统接入后改为读配置,这里是唯一切换点。
const COVER_STORAGE: CoverStorageMode = CoverStorageMode::Raw;

/// 就绪 buffer 类型别名。worker 端 push、client tick 端 drain。
type ReadyBuf = Arc<Mutex<Vec<(MediaUrl, Arc<DynamicImage>)>>>;

/// Client 端封面 fetcher。`spawn` 起 4 worker;`request` 投递;`drain_ready` 拉就绪。
pub struct CoverFetcher {
    /// 待 fetch 的 `(来源, URL)` 队列。worker 从此抢占式拉;来源决定落盘子目录。
    req_tx: mpsc::UnboundedSender<(SourceKind, MediaUrl)>,

    /// worker 完成后塞结果的 buffer;client tick `drain_ready()` 一次拿走。
    ready: ReadyBuf,
}

/// 封面磁盘缓存句柄(可缺):命中省一次网络往返。`None` 表示缓存不可用
/// (目录 / open 失败),直连网络。worker 间共享。
type CoverCache = Option<Arc<CacheIndex>>;

impl CoverFetcher {
    /// 起 4 worker。caller 必须在 tokio runtime 里(`mineral_tui::run` 是 async fn,
    /// 自然满足);失败通常意味着 isahc 客户端建不起来(系统证书 / TLS 问题等)。
    pub async fn spawn() -> color_eyre::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel::<(SourceKind, MediaUrl)>();
        let client = HttpClient::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| eyre!("isahc client init failed: {e}"))?;
        // 磁盘缓存是优化项:目录解析 / open 失败不致命,降级成直连网络不缓存。
        let cache = Self::open_cache().await;
        let ready = Arc::new(Mutex::new(Vec::<(MediaUrl, Arc<DynamicImage>)>::new()));
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..WORKERS {
            let rx = Arc::clone(&rx);
            let ready = Arc::clone(&ready);
            let client = client.clone();
            let cache = cache.clone();
            tokio::spawn(async move {
                worker_loop(rx, ready, client, cache).await;
            });
        }
        Ok(Self { req_tx: tx, ready })
    }

    /// 打开封面磁盘缓存(`cover_cache` 表落 client 的 `tui.db`,文件落 `cover_cache_dir`)。
    /// 目录解析 / open 失败时 warn + 返回 `None`(降级成不缓存),不让 fetcher 起步失败。
    ///
    /// # Return:
    ///   就绪的缓存句柄;不可用时 `None`。
    async fn open_cache() -> CoverCache {
        let (db, dir) = match (mineral_paths::tui_db(), mineral_paths::cover_cache_dir()) {
            (Ok(db), Ok(dir)) => (db, dir),
            (Err(e), _) | (_, Err(e)) => {
                mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "封面缓存路径不可用,降级不缓存");
                return None;
            }
        };
        // sqlite mode=rwc 只建文件不建父目录,fresh env 下需先确保 data_dir 存在。
        if let Some(parent) = db.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            let e = color_eyre::Report::new(e);
            mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "建 tui.db 目录失败,降级不缓存");
            return None;
        }
        let store = match ClientStore::open(&db).await {
            Ok(s) => s,
            Err(e) => {
                mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "打开 tui.db 失败,降级不缓存");
                return None;
            }
        };
        match store.cover_cache(dir, COVER_CACHE_CAPACITY).await {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "封面缓存打开失败,降级不缓存");
                None
            }
        }
    }

    /// 禁用态 fetcher:不起 worker、不建 isahc client,纯 null object。
    ///
    /// 用于封面降级场景——headless / 无网 / isahc 建不起来(TLS / 证书),或测试里
    /// 不需要真抓图时。`request()` 静默丢弃(channel 无人收,send 失败已被忽略),
    /// `drain_ready()` 恒空。与 [`CoverFetcher::spawn`] 不同,**不需要 tokio runtime**。
    pub fn disabled() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel::<(SourceKind, MediaUrl)>();
        Self {
            req_tx: tx,
            ready: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 投递一次 fetch 请求。`source` 决定缓存落盘的子目录(`<source>/...`)。
    pub fn request(&self, source: SourceKind, url: MediaUrl) {
        // worker 全退出时这里 send 失败,忽略 —— 不该发生(tokio task 跟 fetcher
        // 同生命周期),但即使发生也只是「这张图不显示」,不致命。
        let _ = self.req_tx.send((source, url));
    }

    /// 把就绪的图拿走。client 主循环 tick 调一次。
    pub fn drain_ready(&self) -> Vec<(MediaUrl, Arc<DynamicImage>)> {
        std::mem::take(&mut *self.ready.lock())
    }
}

/// fetcher worker 主循环:从队列拉 `(来源, URL)` → 抓 + 解码 + resize → push 到 ready buffer。
async fn worker_loop(
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<(SourceKind, MediaUrl)>>>,
    ready: ReadyBuf,
    client: HttpClient,
    cache: CoverCache,
) {
    loop {
        let (source, url) = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(item) => item,
                None => return, // 队列关了
            }
        };
        if let Some(img) = fetch_and_decode(source, &url, &client, cache.as_ref()).await {
            ready.lock().push((url, Arc::new(img)));
        }
    }
}

/// 取一张封面并解码成内存图,优先磁盘缓存。
///
/// 命中:读盘字节(可能是原图或 resize 成品,都合法)→ 解码。未命中(仅 Remote):下载 →
/// 解码 → 按 [`COVER_STORAGE`] 决定落盘内容写回缓存(`<source>/<hash>.<ext>`)。
/// 解码/缩放/重编码是 CPU 密集活儿,经 [`tokio::task::spawn_blocking`] 落 blocking 池,
/// 不占 runtime worker。Local:直接读盘解码,不进缓存。
///
/// # Params:
///   - `source`: 来源(决定缓存子目录)
///   - `url`: 封面来源 URL
///   - `client`: isahc 客户端(Remote 走它)
///   - `cache`: 磁盘缓存(可缺;`None` 直连不缓存)
///
/// # Return:
///   解码后的图;任一步失败返回 `None`。
async fn fetch_and_decode(
    source: SourceKind,
    url: &MediaUrl,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
) -> Option<DynamicImage> {
    match url {
        MediaUrl::Remote(u) => {
            let key = u.as_str();
            if let Some(bytes) = cached_read(key, cache).await {
                return decode_blocking(url, bytes).await;
            }
            let raw = match download(client, key).await {
                Ok(b) => b,
                Err(e) => {
                    mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&e), "fetch failed");
                    return None;
                }
            };
            let (img, store_bytes, ext) = pack_blocking(url, raw).await?;
            if let Some(cache) = cache {
                store_best_effort(cache, source, key, store_bytes, ext);
            }
            Some(img)
        }
        MediaUrl::Local(p) => {
            let bytes = match tokio::fs::read(p).await {
                Ok(b) => b,
                Err(e) => {
                    let e = color_eyre::Report::new(e);
                    mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&e), "read file failed");
                    return None;
                }
            };
            decode_blocking(url, bytes).await
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

/// 下载 Remote 封面的原始字节。
///
/// # Params:
///   - `client`: isahc 客户端
///   - `key`: 远端 URL
///
/// # Return:
///   原始字节;网络失败返回 `Err`。
async fn download(client: &HttpClient, key: &str) -> color_eyre::Result<Vec<u8>> {
    let mut resp = client
        .get_async(key)
        .await
        .map_err(|e| eyre!("http: {e}"))?;
    resp.bytes().await.map_err(|e| eyre!("read body: {e}"))
}

/// 在 blocking 池解码字节成内存图。
///
/// # Params:
///   - `url`: 仅用于日志
///   - `bytes`: 待解码字节
///
/// # Return:
///   解码后的图;失败返回 `None`(已打日志)。
async fn decode_blocking(url: &MediaUrl, bytes: Vec<u8>) -> Option<DynamicImage> {
    match tokio::task::spawn_blocking(move || decode_resize(&bytes)).await {
        Ok(Ok(img)) => Some(img),
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

/// 同步把字节解码成 image,大图等比 resize 到 `COVER_MAX_DIM` 之内。CPU 密集,由
/// [`fetch_and_decode`] 经 `spawn_blocking` 调,**不要**直接在 async 上下文同步调用。
///
/// # Params:
///   - `bytes`: 封面图原始字节(任意 image 支持的编码)
///
/// # Return:
///   解码并(按需)缩放后的图;解码失败返回 `Err`。
fn decode_resize(bytes: &[u8]) -> color_eyre::Result<DynamicImage> {
    let img = image::load_from_memory(bytes).map_err(|e| eyre!("decode: {e}"))?;
    // resize 到 COVER_MAX_DIM 之内 —— 保持纵横比,Triangle 滤镜质量比 Nearest 好、
    // 比 Lanczos3 快一档,对缩略图够用。原图小于这个就直接用(no-op)。
    let resized = if img.width() > COVER_MAX_DIM || img.height() > COVER_MAX_DIM {
        img.resize(
            COVER_MAX_DIM,
            COVER_MAX_DIM,
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    Ok(resized)
}

/// 在 blocking 池解码 + (按存储模式)算出落盘字节,一次跑完(都是 CPU)。
///
/// # Params:
///   - `url`: 仅用于日志
///   - `raw`: 下载到的原始字节
///
/// # Return:
///   `(内存图, 落盘字节, 扩展名)`;解码 / 编码 / join 失败返回 `None`(已打日志)。
async fn pack_blocking(
    url: &MediaUrl,
    raw: Vec<u8>,
) -> Option<(DynamicImage, Vec<u8>, &'static str)> {
    let packed = tokio::task::spawn_blocking(move || -> color_eyre::Result<_> {
        let img = decode_resize(&raw)?;
        let (store_bytes, ext) = bytes_for_cache(COVER_STORAGE, &raw, &img)?;
        Ok((img, store_bytes, ext))
    })
    .await;
    match packed {
        Ok(Ok(p)) => Some(p),
        Ok(Err(e)) => {
            mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&e), "decode/encode failed");
            None
        }
        Err(e) => {
            mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&e), "decode task join failed");
            None
        }
    }
}

/// best-effort 写回缓存:落盘到 `<source>/<hash>.<ext>`。写文件是重 IO,落 blocking 池;
/// 失败只丢缓存,不影响本次显示。
///
/// # Params:
///   - `cache`: 磁盘缓存
///   - `source`: 来源(子目录)
///   - `key`: 缓存键(= URL 串)
///   - `bytes`: 落盘字节
///   - `ext`: 扩展名
fn store_best_effort(
    cache: &Arc<CacheIndex>,
    source: SourceKind,
    key: &str,
    bytes: Vec<u8>,
    ext: &'static str,
) {
    let cache = Arc::clone(cache);
    let subdir = source.name().to_owned();
    let key = key.to_owned();
    let file_name = cover_file_name(&key, ext);
    // put_bytes 要 await DB 写穿透,内部把写盘下沉到 spawn_blocking;这里用 async task 即可。
    tokio::spawn(async move {
        if let Err(e) = cache.put_bytes(&key, &bytes, &subdir, &file_name).await {
            mineral_log::warn!(target: "cover", key = %key, error = mineral_log::chain(&e), "封面写缓存失败");
        }
    });
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

/// 按存储模式决定落盘字节与扩展名。
///
/// - [`CoverStorageMode::Raw`] → `(原始字节, 嗅探扩展名)`,无损原图。
/// - [`CoverStorageMode::Resized`] → `(重编码 JPEG, "jpg")`,复用已解码的 `img`,不二次解码。
///
/// # Params:
///   - `mode`: 存储模式
///   - `raw`: 原始下载字节
///   - `img`: 已 `decode_resize` 的内存图
///
/// # Return:
///   `(落盘字节, 扩展名)`;JPEG 编码失败返回 `Err`。
fn bytes_for_cache(
    mode: CoverStorageMode,
    raw: &[u8],
    img: &DynamicImage,
) -> color_eyre::Result<(Vec<u8>, &'static str)> {
    match mode {
        CoverStorageMode::Raw => Ok((raw.to_vec(), sniff_ext(raw))),
        CoverStorageMode::Resized => {
            let mut buf = Cursor::new(Vec::<u8>::new());
            let encoder =
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, COVER_JPEG_QUALITY);
            img.write_with_encoder(encoder)
                .map_err(|e| eyre!("jpeg encode: {e}"))?;
            Ok((buf.into_inner(), "jpg"))
        }
    }
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
    use std::io::Cursor;
    use std::sync::Arc;

    use image::{DynamicImage, ImageFormat, RgbImage};
    use mineral_persist::ClientStore;

    use super::{
        COVER_MAX_DIM, CoverStorageMode, bytes_for_cache, cached_read, cover_file_name,
        decode_resize,
    };

    /// 把指定尺寸的纯色 RGB 图编码成 PNG 字节,供 `decode_resize` 测试喂入。
    ///
    /// # Params:
    ///   - `w` / `h`: 目标宽高(像素)
    ///
    /// # Return:
    ///   PNG 编码后的字节;编码失败返回 `Err`。
    fn png_bytes(w: u32, h: u32) -> color_eyre::Result<Vec<u8>> {
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
    fn jpeg_bytes(w: u32, h: u32) -> color_eyre::Result<Vec<u8>> {
        let img = DynamicImage::ImageRgb8(RgbImage::new(w, h));
        let mut buf = Cursor::new(Vec::<u8>::new());
        img.write_to(&mut buf, ImageFormat::Jpeg)
            .map_err(|e| color_eyre::eyre::eyre!("encode jpeg: {e}"))?;
        Ok(buf.into_inner())
    }

    /// Raw 模式:落盘字节 = 原始字节,扩展名按 PNG 字节嗅探为 `png`。
    #[test]
    fn bytes_for_cache_raw_keeps_png() -> color_eyre::Result<()> {
        let png = png_bytes(/*w*/ 10, /*h*/ 10)?;
        let img = decode_resize(&png)?;
        let (bytes, ext) = bytes_for_cache(CoverStorageMode::Raw, &png, &img)?;
        assert_eq!(ext, "png");
        assert_eq!(bytes, png, "Raw 应原样落盘下载字节");
        Ok(())
    }

    /// Raw 模式:JPEG 字节被嗅探为 `jpg`。
    #[test]
    fn bytes_for_cache_raw_sniffs_jpeg() -> color_eyre::Result<()> {
        let jpg = jpeg_bytes(/*w*/ 10, /*h*/ 10)?;
        let img = decode_resize(&jpg)?;
        let (_, ext) = bytes_for_cache(CoverStorageMode::Raw, &jpg, &img)?;
        assert_eq!(ext, "jpg");
        Ok(())
    }

    /// Resized 模式:重编码成 JPEG(`jpg`),解回来尺寸 ≤ `COVER_MAX_DIM`。
    #[test]
    fn bytes_for_cache_resized_reencodes_jpeg_within_max_dim() -> color_eyre::Result<()> {
        let png = png_bytes(/*w*/ 1024, /*h*/ 1024)?;
        let img = decode_resize(&png)?; // 已 resize 到 ≤384
        let (bytes, ext) = bytes_for_cache(CoverStorageMode::Resized, &png, &img)?;
        assert_eq!(ext, "jpg");
        let back = image::load_from_memory(&bytes)
            .map_err(|e| color_eyre::eyre::eyre!("decode back: {e}"))?;
        assert!(
            back.width() <= COVER_MAX_DIM && back.height() <= COVER_MAX_DIM,
            "Resized 落盘图应在上限内,实际 {}x{}",
            back.width(),
            back.height()
        );
        Ok(())
    }

    /// 大图(超过 `COVER_MAX_DIM`)被等比缩到上限之内。
    #[test]
    fn large_image_is_clamped() -> color_eyre::Result<()> {
        let bytes = png_bytes(/*w*/ 1024, /*h*/ 1024)?;
        let img = decode_resize(&bytes)?;
        assert!(
            img.width() <= COVER_MAX_DIM && img.height() <= COVER_MAX_DIM,
            "尺寸应被缩到 {COVER_MAX_DIM} 内,实际 {}x{}",
            img.width(),
            img.height()
        );
        Ok(())
    }

    /// 小图(小于 `COVER_MAX_DIM`)原样返回,不放大、不缩小。
    #[test]
    fn small_image_unchanged() -> color_eyre::Result<()> {
        let bytes = png_bytes(/*w*/ 100, /*h*/ 100)?;
        let img = decode_resize(&bytes)?;
        assert_eq!((img.width(), img.height()), (100, 100));
        Ok(())
    }

    /// 坏字节解码失败返回 `Err`,不 panic。
    #[test]
    fn garbage_bytes_error() {
        assert!(decode_resize(b"not an image").is_err());
    }

    /// 缓存命中时 `cached_read` 直读缓存文件返回字节(结构上不碰网络——它不收 client)。
    #[tokio::test]
    async fn cached_read_hits_disk() -> color_eyre::Result<()> {
        let dir = std::env::temp_dir().join(format!(
            "mineral-cover-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default()
        ));
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
}

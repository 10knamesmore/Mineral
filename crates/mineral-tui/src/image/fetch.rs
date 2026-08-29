//! Client 端的封面源数据预取与按需解码 worker。
//!
//! 若干 tokio worker共享一条结构化请求队列。source warm 只把 Remote 压缩字节落磁盘；
//! decode 请求才在 `spawn_blocking` 中生成完整像素与色板。
//! 跟 mineral-task 的 lane 不同,本 fetcher **归 client 所有** —— 封面是装饰性
//! 资源,server 不该管。多 client 各持一个 fetcher,各 fetch 各 cache。
//!
//! 设计取舍:
//! - **不做 cancel**:用户切走时积压 fetch 仍跑完,结果进 cache 放着;下次显示直接命中。
//!   减一条复杂度,跟现在(server 端 cancel 后的 cache 命中行为)对齐。
//! - **不做内部 dedup**:dedup 由 [`crate::image::ImageEngine`] 的 pending 集合负责。
//!   fetcher 单纯 FIFO worker pool。
//! - **完成态完整**:source warm / decode 成败都会回传 completion，让调用方结束 in-flight。

use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{bail, eyre};
use image::DynamicImage;
use isahc::AsyncReadResponseExt;
use isahc::HttpClient;
use isahc::config::Configurable;
use mineral_config::CoverConfig;
use mineral_model::{MediaUrl, SourceKind};
use mineral_persist::{CacheIndex, ClientStore};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::image::colors::extract_palette;
use crate::render::palette::CoverPalette;

/// worker 完成一张封面的产物:图必有,色板尽力而为。
///
/// `ReadyBuf` 的元素从元组升成结构体,遵守"跨边界优先结构化"约定;`palette` 为
/// `Option` 让取色失败不挡封面图本身回传(取色是封面的附属信息)。
pub(crate) struct CoverReady {
    /// 封面来源 URL(= 缓存键 / drain 回填键)。
    pub url: MediaUrl,

    /// 解码后的内存图。
    pub image: Arc<DynamicImage>,

    /// 从图提取的频谱色板;取色失败为 `None`(频谱回退 hue 漂移)。
    pub palette: Option<CoverPalette>,

    /// Remote 压缩源数据是否已确认写入或命中磁盘缓存。
    pub source_cached: bool,
}

/// 解码产物:内存图 + 尽力而为的频谱色板。一次 `spawn_blocking` 内算完(都是 CPU 活儿)。
struct DecodedCover {
    /// 解码后的内存图。
    image: DynamicImage,

    /// 从图提取的频谱色板(取色失败为 `None`)。
    palette: Option<CoverPalette>,
}

/// blocking 解码产物：内存图与仍归调用方所有的原始压缩字节。
struct DecodedBytes {
    /// 解码 + 取色结果。
    decoded: DecodedCover,

    /// 写回磁盘缓存的原始压缩字节。
    source_bytes: Vec<u8>,
}

/// 一次按需解码的完整产物。
struct LoadedCover {
    /// 解码图与色板。
    decoded: DecodedCover,

    /// Remote 压缩源数据是否已确认位于磁盘缓存。
    source_cached: bool,
}

/// 图片 worker 请求类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoverRequestKind {
    /// 只把 Remote 压缩源数据准备到磁盘缓存。
    WarmSource,

    /// 读取本地源数据并生成完整解码图与色板。
    Decode,
}

/// 一次图片 worker 请求。
struct CoverRequest {
    /// 图片来源，决定 Remote 文件的缓存子目录。
    source: SourceKind,

    /// 图片源 URL。
    url: MediaUrl,

    /// 本次请求只预取源数据还是需要解码。
    kind: CoverRequestKind,
}

/// 一次图片 worker 完成事件。
pub(crate) enum CoverCompletion {
    /// Remote 压缩源数据已经位于磁盘缓存。
    SourceReady {
        /// 已准备完成的图片 URL。
        url: MediaUrl,
    },

    /// 图片已解码，可以进入 RAM LRU。
    Decoded(CoverReady),

    /// 请求失败；错误已在 worker 边界记录。
    Failed {
        /// 失败的图片 URL。
        url: MediaUrl,

        /// 失败请求的类型。
        kind: CoverRequestKind,
    },
}

/// 完成 buffer 类型别名。worker 端 push、client tick 端 drain。
type ReadyBuf = Arc<Mutex<Vec<CoverCompletion>>>;

/// Client 端封面 fetcher。`spawn` 起 worker 池;`request` 投递;`drain_ready` 拉就绪。
pub(crate) struct CoverFetcher {
    /// 待执行的 source warm / decode 请求队列。
    req_tx: mpsc::UnboundedSender<CoverRequest>,

    /// worker 完成后塞结果的 buffer;client tick `drain_ready()` 一次拿走。
    ready: ReadyBuf,

    /// 禁用态保留接收端，让既有无 runtime fixture 可以观察已提交请求。
    _disabled_rx: Option<mpsc::UnboundedReceiver<CoverRequest>>,
}

/// 封面磁盘缓存句柄(可缺):命中省一次网络往返。`None` 表示缓存不可用
/// (目录 / open 失败),直连网络。worker 间共享。
type CoverCache = Option<Arc<CacheIndex>>;

impl CoverFetcher {
    /// 起 worker 池(数量 = `cfg.download_workers`)。caller 必须在 tokio runtime 里
    /// (`mineral_tui::run` 是 async fn,自然满足);失败通常意味着 isahc 客户端建不起来
    /// (系统证书 / TLS 问题等)。
    ///
    /// # Params:
    ///   - `cfg`: 封面段配置(timeout / 并发 / kmeans)
    ///   - `cover_capacity`: 封面磁盘缓存容量上限(字节,配置 `tui.cover.cache.disk`)
    ///   - `store`: 共享的 `tui.db` 句柄(与 UI 偏好共用连接池;`None` = 降级不缓存)
    pub(crate) async fn spawn(
        cfg: CoverConfig,
        cover_capacity: u64,
        store: Option<Arc<ClientStore>>,
    ) -> color_eyre::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel::<CoverRequest>();
        let client = HttpClient::builder()
            .timeout(Duration::from_secs(*cfg.http_timeout_secs()))
            .build()
            .map_err(|e| eyre!("isahc client init failed: {e}"))?;
        // 磁盘缓存是优化项:store 不可用 / 目录解析失败不致命,降级成直连网络不缓存。
        let cache = Self::open_cache(store, cover_capacity).await;
        let ready = Arc::new(Mutex::new(Vec::<CoverCompletion>::new()));
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        let cfg = Arc::new(cfg);
        for _ in 0..(*cfg.download_workers()).max(1) {
            let rx = Arc::clone(&rx);
            let ready = Arc::clone(&ready);
            let client = client.clone();
            let cache = cache.clone();
            let cfg = Arc::clone(&cfg);
            tokio::spawn(async move {
                worker_loop(rx, ready, client, cache, cfg).await;
            });
        }
        Ok(Self {
            req_tx: tx,
            ready,
            _disabled_rx: None,
        })
    }

    /// 打开封面磁盘缓存(`cover_cache` 表落共享的 `tui.db`,文件落 `cover_cache_dir`)。
    /// store 不可用 / 目录解析 / open 失败时 warn + 返回 `None`(降级成不缓存),
    /// 不让 fetcher 起步失败。
    ///
    /// # Params:
    ///   - `store`: 共享的 `tui.db` 句柄(`None` = 上游已降级)
    ///   - `capacity`: 缓存容量上限(字节,配置 `tui.cover.cache.disk`)
    ///
    /// # Return:
    ///   就绪的缓存句柄;不可用时 `None`。
    async fn open_cache(store: Option<Arc<ClientStore>>, capacity: u64) -> CoverCache {
        let store = store?;
        let dir = match mineral_paths::cover_cache_dir() {
            Ok(dir) => dir,
            Err(e) => {
                mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "封面缓存目录不可用,降级不缓存");
                return None;
            }
        };
        match store.cover_cache(dir, capacity).await {
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
    pub(crate) fn disabled() -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<CoverRequest>();
        Self {
            req_tx: tx,
            ready: Arc::new(Mutex::new(Vec::new())),
            _disabled_rx: Some(rx),
        }
    }

    /// 投递一次 Remote 压缩源数据预取请求。
    ///
    /// # Params:
    ///   - `source`: 来源，决定缓存子目录
    ///   - `url`: 图片源 URL
    ///
    /// # Return:
    ///   worker 队列仍可接收请求时返回 `true`
    pub(crate) fn warm_source(&self, source: SourceKind, url: MediaUrl) -> bool {
        self.request(source, url, CoverRequestKind::WarmSource)
    }

    /// 投递一次完整图片解码请求。
    ///
    /// # Params:
    ///   - `source`: 来源，决定 Remote 缓存子目录
    ///   - `url`: 图片源 URL
    ///
    /// # Return:
    ///   worker 队列仍可接收请求时返回 `true`
    pub(crate) fn decode(&self, source: SourceKind, url: MediaUrl) -> bool {
        self.request(source, url, CoverRequestKind::Decode)
    }

    /// 把全部图片 worker completion 拿走。client 主循环 tick 调一次。
    pub(crate) fn drain_ready(&self) -> Vec<CoverCompletion> {
        std::mem::take(&mut *self.ready.lock())
    }

    /// 向 worker 队列投递一个结构化请求。
    ///
    /// # Params:
    ///   - `source`: 图片来源
    ///   - `url`: 图片源 URL
    ///   - `kind`: source warm 或 decode
    ///
    /// # Return:
    ///   请求是否进入队列
    fn request(&self, source: SourceKind, url: MediaUrl, kind: CoverRequestKind) -> bool {
        self.req_tx.send(CoverRequest { source, url, kind }).is_ok()
    }
}

/// worker 主循环：串行领取请求，完成 source warm 或 decode 后推送完整 completion。
async fn worker_loop(
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<CoverRequest>>>,
    ready: ReadyBuf,
    client: HttpClient,
    cache: CoverCache,
    cfg: Arc<CoverConfig>,
) {
    loop {
        let request = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(item) => item,
                None => return, // 队列关了
            }
        };
        let completion = complete_request(request, &client, cache.as_ref(), &cfg).await;
        ready.lock().push(completion);
    }
}

/// 执行一个结构化图片请求并生成必达 completion。
///
/// # Params:
///   - `request`: source warm 或 decode 请求
///   - `client`: Remote 图片 HTTP 客户端
///   - `cache`: 可用的磁盘缓存
///   - `cfg`: 解码与取色配置
///
/// # Return:
///   成功产物或带请求类型的失败 completion
async fn complete_request(
    request: CoverRequest,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
    cfg: &Arc<CoverConfig>,
) -> CoverCompletion {
    let CoverRequest { source, url, kind } = request;
    match kind {
        CoverRequestKind::WarmSource => {
            if warm_remote_source(source, &url, client, cache).await {
                CoverCompletion::SourceReady { url }
            } else {
                CoverCompletion::Failed { url, kind }
            }
        }
        CoverRequestKind::Decode => {
            if let Some(loaded) = fetch_and_decode(source, &url, client, cache, cfg).await {
                CoverCompletion::Decoded(CoverReady {
                    url,
                    image: Arc::new(loaded.decoded.image),
                    palette: loaded.decoded.palette,
                    source_cached: loaded.source_cached,
                })
            } else {
                CoverCompletion::Failed { url, kind }
            }
        }
    }
}

/// 把 Remote 图片压缩源数据准备到磁盘，不执行解码或取色。
///
/// # Params:
///   - `source`: 来源，决定缓存子目录
///   - `url`: 图片源 URL
///   - `client`: HTTP 客户端
///   - `cache`: 磁盘缓存；不可用时无法完成预取
///
/// # Return:
///   源数据已经命中或成功写入磁盘缓存时返回 `true`
async fn warm_remote_source(
    source: SourceKind,
    url: &MediaUrl,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
) -> bool {
    let MediaUrl::Remote(remote) = url else {
        return true;
    };
    let Some(cache) = cache else {
        return false;
    };
    let key = remote.as_str();
    if cache.get(key).is_some() {
        return true;
    }
    let started = std::time::Instant::now();
    let raw = match download(client, key).await {
        Ok(bytes) => bytes,
        Err(error) => {
            mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&error), "fetch failed");
            return false;
        }
    };
    log_downloaded(key, started, raw.len());
    store_source(cache, source, key, &raw).await
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
///
/// # Return:
///   解码后的图 + 色板;任一步失败返回 `None`。
async fn fetch_and_decode(
    source: SourceKind,
    url: &MediaUrl,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
    cfg: &Arc<CoverConfig>,
) -> Option<LoadedCover> {
    match url {
        MediaUrl::Remote(u) => {
            let key = u.as_str();
            if let Some(bytes) = cached_read(key, cache).await {
                return decode_blocking(url, bytes, cfg)
                    .await
                    .map(|decoded| LoadedCover {
                        decoded: decoded.decoded,
                        source_cached: true,
                    });
            }
            let started = std::time::Instant::now();
            let raw = match download(client, key).await {
                Ok(b) => b,
                Err(e) => {
                    mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&e), "fetch failed");
                    return None;
                }
            };
            log_downloaded(key, started, raw.len());
            let DecodedBytes {
                decoded,
                source_bytes,
            } = decode_blocking(url, raw, cfg).await?;
            let source_cached = if let Some(cache) = cache {
                store_source(cache, source, key, &source_bytes).await
            } else {
                false
            };
            Some(LoadedCover {
                decoded,
                source_cached,
            })
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
            decode_blocking(url, bytes, cfg)
                .await
                .map(|decoded| LoadedCover {
                    decoded: decoded.decoded,
                    source_cached: true,
                })
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

/// 在 blocking 池解码字节成内存图。
///
/// # Params:
///   - `url`: 仅用于日志
///   - `bytes`: 待解码字节
///   - `cfg`: 封面段配置(kmeans)
///
/// # Return:
///   解码后的图、色板与原始压缩字节；失败返回 `None`(已打日志)。
async fn decode_blocking(
    url: &MediaUrl,
    bytes: Vec<u8>,
    cfg: &Arc<CoverConfig>,
) -> Option<DecodedBytes> {
    let cfg = Arc::clone(cfg);
    let decoded = tokio::task::spawn_blocking(move || -> color_eyre::Result<DecodedBytes> {
        let image = decode(&bytes)?;
        let palette = extract_palette(&image, cfg.kmeans());
        Ok(DecodedBytes {
            decoded: DecodedCover { image, palette },
            source_bytes: bytes,
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

/// 同步把压缩字节解码成 image。CPU 密集，由 [`decode_blocking`] 在 blocking pool 调用。
///
/// # Params:
///   - `bytes`: 封面图原始字节(任意 image 支持的编码)
/// # Return:
///   解码后的完整图片；解码失败返回 `Err`。
fn decode(bytes: &[u8]) -> color_eyre::Result<DynamicImage> {
    image::load_from_memory(bytes).map_err(|e| eyre!("decode: {e}"))
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
    use std::io::Cursor;
    use std::sync::Arc;

    use image::{DynamicImage, ImageFormat, RgbImage};
    use mineral_persist::ClientStore;

    use super::{cached_read, cover_file_name, decode, download, sniff_ext};

    /// PID + 纳秒后缀的唯一临时目录。
    fn temp_dir() -> std::path::PathBuf {
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

    /// decode 保留源图片尺寸，不做全局缩放。
    #[test]
    fn decode_keeps_source_dimensions() -> color_eyre::Result<()> {
        let bytes = png_bytes(/*w*/ 1024, /*h*/ 1024)?;
        let decoded = decode(&bytes)?;
        assert_eq!((decoded.width(), decoded.height()), (1024, 1024));
        Ok(())
    }

    /// 坏字节解码失败返回 `Err`,不 panic。
    #[test]
    fn garbage_bytes_error() {
        assert!(decode(b"not an image").is_err());
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

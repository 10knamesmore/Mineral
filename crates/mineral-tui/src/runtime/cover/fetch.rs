//! Client 端的封面图 fetcher。
//!
//! 若干 tokio worker(数量 = 配置 `cover.download_workers`)共享一条 mpsc 队列:worker 上 async 抓字节,解码+resize 经
//! `spawn_blocking` 落到 blocking 线程池(CPU 密集,别占 runtime worker)。
//! 跟 mineral-task 的 lane 不同,本 fetcher **归 client 所有** —— 封面是装饰性
//! 资源,server 不该管。多 client 各持一个 fetcher,各 fetch 各 cache。
//!
//! 设计取舍:
//! - **不做 cancel**:用户切走时积压 fetch 仍跑完,结果进 cache 放着;下次显示直接命中。
//!   减一条复杂度,跟现在(server 端 cancel 后的 cache 命中行为)对齐。
//! - **不做内部 dedup**:dedup 由 caller(`prefetch::ensure_cover` 用 `state.covers.pending`
//!   集合)做。fetcher 单纯 FIFO worker pool。
//! - **错误静默**:fetch / decode 失败只打日志,不推 result,UI 自然显示 fallback。

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::{bail, eyre};
use image::DynamicImage;
use isahc::AsyncReadResponseExt;
use isahc::HttpClient;
use isahc::config::Configurable;
use mineral_config::{CoverConfig, CoverStorageMode};
use mineral_model::{MediaUrl, SourceKind};
use mineral_persist::{CacheIndex, ClientStore};
use parking_lot::Mutex;
use tokio::sync::mpsc;

use crate::render::palette::CoverPalette;
use crate::runtime::cover::cdn_scale::scaled_url;
use crate::runtime::cover::colors::extract_palette;

/// worker 完成一张封面的产物:图必有,色板尽力而为。
///
/// `ReadyBuf` 的元素从元组升成结构体,遵守"跨边界优先结构化"约定;`palette` 为
/// `Option` 让取色失败不挡封面图本身回传(取色是封面的附属信息)。
pub struct CoverReady {
    /// 封面来源 URL(= 缓存键 / drain 回填键)。
    pub url: MediaUrl,

    /// 解码 + resize 后的内存图(≤ 配置 `cover.max_dim`)。
    pub image: Arc<DynamicImage>,

    /// 从图提取的频谱色板;取色失败为 `None`(频谱回退 hue 漂移)。
    pub palette: Option<CoverPalette>,
}

/// 解码产物:内存图 + 尽力而为的频谱色板。一次 `spawn_blocking` 内算完(都是 CPU 活儿)。
struct DecodedCover {
    /// 解码 + resize 后的内存图。
    image: DynamicImage,

    /// 从图提取的频谱色板(取色失败为 `None`)。
    palette: Option<CoverPalette>,

    /// 解码时原图被缩过(见 [`DecodedImage::clamped`];自愈回写判定用)。
    clamped: bool,
}

/// `pack_blocking` 的产物:解码结果 + 写回缓存的字节 + 扩展名。命名字段替代三元组。
struct PackedCover {
    /// 解码 + 取色结果。
    decoded: DecodedCover,

    /// 写回磁盘缓存的字节(按配置 `cover.storage` 决定原图 / 重编码)。
    store_bytes: Vec<u8>,

    /// 缓存文件扩展名。
    ext: &'static str,
}

/// 就绪 buffer 类型别名。worker 端 push、client tick 端 drain。
type ReadyBuf = Arc<Mutex<Vec<CoverReady>>>;

/// Client 端封面 fetcher。`spawn` 起 worker 池;`request` 投递;`drain_ready` 拉就绪。
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
    /// 起 worker 池(数量 = `cfg.download_workers`)。caller 必须在 tokio runtime 里
    /// (`mineral_tui::run` 是 async fn,自然满足);失败通常意味着 isahc 客户端建不起来
    /// (系统证书 / TLS 问题等)。
    ///
    /// # Params:
    ///   - `cfg`: 封面段配置(timeout / 尺寸 / 存储形态 / 并发 / kmeans)
    ///   - `cover_capacity`: 封面磁盘缓存容量上限(字节,配置 `cache.cover_capacity`)
    ///   - `store`: 共享的 `tui.db` 句柄(与 UI 偏好共用连接池;`None` = 降级不缓存)
    pub async fn spawn(
        cfg: CoverConfig,
        cover_capacity: u64,
        store: Option<Arc<ClientStore>>,
    ) -> color_eyre::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel::<(SourceKind, MediaUrl)>();
        let client = HttpClient::builder()
            .timeout(Duration::from_secs(*cfg.http_timeout_secs()))
            .build()
            .map_err(|e| eyre!("isahc client init failed: {e}"))?;
        // 磁盘缓存是优化项:store 不可用 / 目录解析失败不致命,降级成直连网络不缓存。
        let cache = Self::open_cache(store, cover_capacity).await;
        let ready = Arc::new(Mutex::new(Vec::<CoverReady>::new()));
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
        Ok(Self { req_tx: tx, ready })
    }

    /// 打开封面磁盘缓存(`cover_cache` 表落共享的 `tui.db`,文件落 `cover_cache_dir`)。
    /// store 不可用 / 目录解析 / open 失败时 warn + 返回 `None`(降级成不缓存),
    /// 不让 fetcher 起步失败。
    ///
    /// # Params:
    ///   - `store`: 共享的 `tui.db` 句柄(`None` = 上游已降级)
    ///   - `capacity`: 缓存容量上限(字节,配置 `cache.cover_capacity`)
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

    /// 把就绪的封面(图 + 色板)拿走。client 主循环 tick 调一次。
    pub fn drain_ready(&self) -> Vec<CoverReady> {
        std::mem::take(&mut *self.ready.lock())
    }
}

/// fetcher worker 主循环:从队列拉 `(来源, URL)` → 抓 + 解码 + resize → push 到 ready buffer。
async fn worker_loop(
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<(SourceKind, MediaUrl)>>>,
    ready: ReadyBuf,
    client: HttpClient,
    cache: CoverCache,
    cfg: Arc<CoverConfig>,
) {
    loop {
        let (source, url) = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(item) => item,
                None => return, // 队列关了
            }
        };
        if let Some(decoded) = fetch_and_decode(source, &url, &client, cache.as_ref(), &cfg).await {
            ready.lock().push(CoverReady {
                url,
                image: Arc::new(decoded.image),
                palette: decoded.palette,
            });
        }
    }
}

/// 取一张封面并解码成内存图,优先磁盘缓存。
///
/// 命中:读盘字节(可能是原图或 resize 成品,都合法)→ 解码。未命中(仅 Remote):下载 →
/// 解码 → 按配置 `cover.storage` 决定落盘内容写回缓存(`<source>/<hash>.<ext>`)。
/// 解码/缩放/重编码是 CPU 密集活儿,经 [`tokio::task::spawn_blocking`] 落 blocking 池,
/// 不占 runtime worker。Local:直接读盘解码,不进缓存。
///
/// # Params:
///   - `source`: 来源(决定缓存子目录)
///   - `url`: 封面来源 URL
///   - `client`: isahc 客户端(Remote 走它)
///   - `cache`: 磁盘缓存(可缺;`None` 直连不缓存)
///   - `cfg`: 封面段配置(尺寸 / 存储形态 / kmeans)
///
/// # Return:
///   解码后的图 + 色板;任一步失败返回 `None`。
async fn fetch_and_decode(
    source: SourceKind,
    url: &MediaUrl,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
    cfg: &Arc<CoverConfig>,
) -> Option<DecodedCover> {
    match url {
        MediaUrl::Remote(u) => {
            let key = u.as_str();
            if let Some(bytes) = cached_read(key, cache).await {
                let decoded = decode_blocking(url, bytes, cfg).await?;
                // 旧缓存自愈:Raw 时代存的原图(解码时被缩过)在 Resized 模式下重编码
                // 覆盖回写同 key,每条至多触发一次(升级后解码不再 clamped)。
                // ⚠️ 同 key 原地覆盖复用原 relpath,旧扩展名不变(读回靠字节嗅探,无害)。
                if decoded.clamped
                    && matches!(*cfg.storage(), CoverStorageMode::Resized)
                    && let Some(cache) = cache
                {
                    upgrade_cached_best_effort(
                        cache,
                        source,
                        key,
                        decoded.image.clone(),
                        *cfg.jpeg_quality(),
                    );
                }
                return Some(decoded);
            }
            let raw = match download_preferring(client, scaled_url(u, *cfg.max_dim()), key).await {
                Ok(b) => b,
                Err(e) => {
                    mineral_log::warn!(target: "cover", url = %url, error = mineral_log::chain(&e), "fetch failed");
                    return None;
                }
            };
            let packed = pack_blocking(url, raw, cfg).await?;
            if let Some(cache) = cache {
                store_best_effort(cache, source, key, packed.store_bytes, packed.ext);
            }
            Some(packed.decoded)
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
            decode_blocking(url, bytes, cfg).await
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

/// 下载封面字节:有图床缩放地址先试它,失败(网络/非 2xx)回退原始 URL。
///
/// 缩放地址是尽力而为的传输优化(图床语法可能变更/个别对象可能不支持),
/// 失败只降级不报错;原始 URL 也失败才向上返回 `Err`。
///
/// # Params:
///   - `client`: isahc 客户端
///   - `scaled`: 图床缩放下载地址([`scaled_url`] 产物;`None` 直接用原始 URL)
///   - `canonical`: 原始 URL(= 缓存键)
///
/// # Return:
///   封面原始字节;可用地址全失败返回 `Err`。
async fn download_preferring(
    client: &HttpClient,
    scaled: Option<String>,
    canonical: &str,
) -> color_eyre::Result<Vec<u8>> {
    let had_scaled = scaled.is_some();
    if let Some(scaled) = scaled {
        let started = std::time::Instant::now();
        match download(client, &scaled).await {
            Ok(bytes) => {
                log_downloaded("scaled", &scaled, started, bytes.len());
                return Ok(bytes);
            }
            Err(e) => {
                mineral_log::warn!(target: "cover", url = %scaled, error = mineral_log::chain(&e), "缩放地址下载失败,回退原始 URL");
            }
        }
    }
    let started = std::time::Instant::now();
    let bytes = download(client, canonical).await?;
    let route = if had_scaled { "fallback" } else { "direct" };
    log_downloaded(route, canonical, started, bytes.len());
    Ok(bytes)
}

/// 打一条封面下载完成的 debug 日志(耗时/字节数/走的哪条路)。
///
/// 排查「封面变慢」类问题的一手数据:`RUST_LOG=cover=debug` 打开。
///
/// # Params:
///   - `route`: `scaled`(缩放地址)/ `fallback`(缩放失败回退原始)/ `direct`(无缩放地址)
///   - `url`: 实际下载用的地址
///   - `started`: 本次请求起点
///   - `bytes`: 响应体大小
fn log_downloaded(route: &str, url: &str, started: std::time::Instant, bytes: usize) {
    let elapsed_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    mineral_log::debug!(target: "cover", %route, %url, elapsed_ms, bytes, "封面下载完成");
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
///   - `cfg`: 封面段配置(resize 上限 + kmeans)
///
/// # Return:
///   解码后的图 + 色板;失败返回 `None`(已打日志)。
async fn decode_blocking(
    url: &MediaUrl,
    bytes: Vec<u8>,
    cfg: &Arc<CoverConfig>,
) -> Option<DecodedCover> {
    let cfg = Arc::clone(cfg);
    let decoded = tokio::task::spawn_blocking(move || -> color_eyre::Result<DecodedCover> {
        let DecodedImage { image, clamped } = decode_resize(&bytes, *cfg.max_dim())?;
        let palette = extract_palette(&image, cfg.kmeans());
        Ok(DecodedCover {
            image,
            palette,
            clamped,
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

/// [`decode_resize`] 的产物:内存图 + 是否真的缩过(供旧缓存自愈判定)。
struct DecodedImage {
    /// 解码并(按需)缩放后的图。
    image: DynamicImage,

    /// 原图超过 `max_dim` 被缩过。`true` 说明缓存里躺的还是大图(Raw 时代存量),
    /// Resized 模式下触发一次重编码回写自愈。
    clamped: bool,
}

/// 同步把字节解码成 image,大图等比 resize 到 `max_dim` 之内。CPU 密集,由
/// [`fetch_and_decode`] 经 `spawn_blocking` 调,**不要**直接在 async 上下文同步调用。
///
/// # Params:
///   - `bytes`: 封面图原始字节(任意 image 支持的编码)
///   - `max_dim`: resize 最大边(像素,配置 `cover.max_dim`)
///
/// # Return:
///   解码并(按需)缩放后的图 + 是否缩过;解码失败返回 `Err`。
fn decode_resize(bytes: &[u8], max_dim: u32) -> color_eyre::Result<DecodedImage> {
    let img = image::load_from_memory(bytes).map_err(|e| eyre!("decode: {e}"))?;
    // resize 到 max_dim 之内 —— 保持纵横比,Triangle 滤镜质量比 Nearest 好、
    // 比 Lanczos3 快一档,对缩略图够用。原图小于这个就直接用(no-op)。
    let clamped = img.width() > max_dim || img.height() > max_dim;
    let image = if clamped {
        img.resize(max_dim, max_dim, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Ok(DecodedImage { image, clamped })
}

/// 在 blocking 池解码 + (按存储模式)算出落盘字节,一次跑完(都是 CPU)。
///
/// # Params:
///   - `url`: 仅用于日志
///   - `raw`: 下载到的原始字节
///   - `cfg`: 封面段配置(resize 上限 / 存储形态 / JPEG 质量 / kmeans)
///
/// # Return:
///   [`PackedCover`](内存图 + 色板 + 落盘字节 + 扩展名);解码 / 编码 / join 失败返回 `None`(已打日志)。
async fn pack_blocking(
    url: &MediaUrl,
    raw: Vec<u8>,
    cfg: &Arc<CoverConfig>,
) -> Option<PackedCover> {
    let cfg = Arc::clone(cfg);
    let packed = tokio::task::spawn_blocking(move || -> color_eyre::Result<PackedCover> {
        let DecodedImage { image, clamped } = decode_resize(&raw, *cfg.max_dim())?;
        let (store_bytes, ext) =
            bytes_for_cache(*cfg.storage(), &raw, &image, *cfg.jpeg_quality())?;
        let palette = extract_palette(&image, cfg.kmeans());
        Ok(PackedCover {
            decoded: DecodedCover {
                image,
                palette,
                clamped,
            },
            store_bytes,
            ext,
        })
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
///   - `mode`: 存储模式(配置 `cover.storage`)
///   - `raw`: 原始下载字节
///   - `img`: 已 `decode_resize` 的内存图
///   - `jpeg_quality`: Resized 模式重编码 JPEG 的质量(0–100,配置 `cover.jpeg_quality`)
///
/// # Return:
///   `(落盘字节, 扩展名)`;JPEG 编码失败返回 `Err`。
fn bytes_for_cache(
    mode: CoverStorageMode,
    raw: &[u8],
    img: &DynamicImage,
    jpeg_quality: u8,
) -> color_eyre::Result<(Vec<u8>, &'static str)> {
    match mode {
        CoverStorageMode::Resized => Ok((encode_jpeg(img, jpeg_quality)?, "jpg")),
        // CoverStorageMode 是 #[non_exhaustive]:未来新形态接线前按 Raw(无损)兜底。
        CoverStorageMode::Raw | _ => Ok((raw.to_vec(), sniff_ext(raw))),
    }
}

/// 把内存图重编码成 JPEG 字节(Resized 落盘 / 旧缓存自愈共用)。
///
/// # Params:
///   - `img`: 已解码(≤ max_dim)的内存图
///   - `quality`: JPEG 质量 1-100(配置 `cover.jpeg_quality`)
///
/// # Return:
///   JPEG 字节;编码失败返回 `Err`。
fn encode_jpeg(img: &DynamicImage, quality: u8) -> color_eyre::Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::<u8>::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
    img.write_with_encoder(encoder)
        .map_err(|e| eyre!("jpeg encode: {e}"))?;
    Ok(buf.into_inner())
}

/// 旧缓存自愈:把已解码的(≤ max_dim)图重编码 JPEG 后覆盖回写同 key。
/// 编码落 blocking 池、写盘走 [`store_best_effort`];任一步失败只 warn,
/// 本次显示不受影响(下次命中再试)。
///
/// # Params:
///   - `cache`: 磁盘缓存
///   - `source`: 来源(子目录;同 key 覆盖时实际复用原 relpath)
///   - `key`: 缓存键(= URL 串)
///   - `image`: 已解码缩放的图(clone 进后台,一次性自愈可接受)
///   - `jpeg_quality`: JPEG 质量(配置 `cover.jpeg_quality`)
fn upgrade_cached_best_effort(
    cache: &Arc<CacheIndex>,
    source: SourceKind,
    key: &str,
    image: DynamicImage,
    jpeg_quality: u8,
) {
    let cache = Arc::clone(cache);
    let key_owned = key.to_owned();
    tokio::spawn(async move {
        let encoded = tokio::task::spawn_blocking(move || encode_jpeg(&image, jpeg_quality)).await;
        match encoded {
            Ok(Ok(bytes)) => {
                mineral_log::debug!(target: "cover", key = %key_owned, bytes = bytes.len(), "旧缓存自愈:重编码回写");
                store_best_effort(&cache, source, &key_owned, bytes, /*ext*/ "jpg");
            }
            Ok(Err(e)) => {
                mineral_log::warn!(target: "cover", key = %key_owned, error = mineral_log::chain(&e), "自愈重编码失败");
            }
            Err(e) => {
                mineral_log::warn!(target: "cover", key = %key_owned, error = mineral_log::chain(&e), "自愈编码 task join 失败");
            }
        }
    });
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
    use mineral_config::CoverConfig;
    use mineral_model::{MediaUrl, SourceKind};
    use mineral_persist::ClientStore;

    use super::{
        CoverStorageMode, bytes_for_cache, cached_read, cover_file_name, decode_resize, download,
        download_preferring, fetch_and_decode,
    };

    /// 测试用封面段配置(storage 可选;其余为测试基线值,生产默认见 default.lua)。
    fn cover_cfg(storage: &str) -> color_eyre::Result<Arc<CoverConfig>> {
        let cfg: CoverConfig = serde_json::from_value(serde_json::json!({
            "http_timeout_secs": 30, "max_dim": 384, "jpeg_quality": 85,
            "storage": storage, "debounce_ms": 80,
            "download_workers": 1, "encode_workers": 1,
            "kmeans": {
                "sample_dim": 64, "swatches": 6, "seed": 1, "max_iter": 20, "converge": 5.0,
                "l_min": 8.0, "l_max": 92.0, "chroma_min": 8.0, "min_valid_pixels_pct": 5,
            },
        }))?;
        Ok(Arc::new(cfg))
    }

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

    /// 测试入参:resize 上限(= default.lua 的 `cover.max_dim`)。
    const COVER_MAX_DIM: u32 = 384;

    /// 测试入参:Resized 模式 JPEG 质量(任意合法值即可,函数行为与具体值无关)。
    const COVER_JPEG_QUALITY: u8 = 85;

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
        let img = decode_resize(&png, COVER_MAX_DIM)?.image;
        let (bytes, ext) = bytes_for_cache(CoverStorageMode::Raw, &png, &img, COVER_JPEG_QUALITY)?;
        assert_eq!(ext, "png");
        assert_eq!(bytes, png, "Raw 应原样落盘下载字节");
        Ok(())
    }

    /// Raw 模式:JPEG 字节被嗅探为 `jpg`。
    #[test]
    fn bytes_for_cache_raw_sniffs_jpeg() -> color_eyre::Result<()> {
        let jpg = jpeg_bytes(/*w*/ 10, /*h*/ 10)?;
        let img = decode_resize(&jpg, COVER_MAX_DIM)?.image;
        let (_, ext) = bytes_for_cache(CoverStorageMode::Raw, &jpg, &img, COVER_JPEG_QUALITY)?;
        assert_eq!(ext, "jpg");
        Ok(())
    }

    /// Resized 模式:重编码成 JPEG(`jpg`),解回来尺寸 ≤ `COVER_MAX_DIM`。
    #[test]
    fn bytes_for_cache_resized_reencodes_jpeg_within_max_dim() -> color_eyre::Result<()> {
        let png = png_bytes(/*w*/ 1024, /*h*/ 1024)?;
        let img = decode_resize(&png, COVER_MAX_DIM)?.image; // 已 resize 到 ≤384
        let (bytes, ext) =
            bytes_for_cache(CoverStorageMode::Resized, &png, &img, COVER_JPEG_QUALITY)?;
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

    /// 大图(超过 `COVER_MAX_DIM`)被等比缩到上限之内,并带 `clamped` 标记(自愈判定源)。
    #[test]
    fn large_image_is_clamped() -> color_eyre::Result<()> {
        let bytes = png_bytes(/*w*/ 1024, /*h*/ 1024)?;
        let decoded = decode_resize(&bytes, COVER_MAX_DIM)?;
        assert!(decoded.clamped, "1024² 应标记 clamped");
        assert!(
            decoded.image.width() <= COVER_MAX_DIM && decoded.image.height() <= COVER_MAX_DIM,
            "尺寸应被缩到 {COVER_MAX_DIM} 内,实际 {}x{}",
            decoded.image.width(),
            decoded.image.height()
        );
        Ok(())
    }

    /// 小图(小于 `COVER_MAX_DIM`)原样返回,不放大、不缩小,且不标 `clamped`。
    #[test]
    fn small_image_unchanged() -> color_eyre::Result<()> {
        let bytes = png_bytes(/*w*/ 100, /*h*/ 100)?;
        let decoded = decode_resize(&bytes, COVER_MAX_DIM)?;
        assert!(!decoded.clamped, "300² 以下不应标 clamped");
        assert_eq!((decoded.image.width(), decoded.image.height()), (100, 100));
        Ok(())
    }

    /// 坏字节解码失败返回 `Err`,不 panic。
    #[test]
    fn garbage_bytes_error() {
        assert!(decode_resize(b"not an image", COVER_MAX_DIM).is_err());
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

    /// 旧缓存自愈:Raw 时代存的 1024² 原图,在 Resized 模式下命中一次后被重编码
    /// ≤max_dim JPEG 覆盖回写;再命中不再触发回写(字节不变)。
    /// 自愈在后台 task + blocking 池跑,需 multi_thread runtime + 轮询等待。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stale_raw_cache_entry_self_heals_to_resized() -> color_eyre::Result<()> {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir)?;
        let store = ClientStore::open(&dir.join("cover.db")).await?;
        let cache = Arc::new(
            store
                .cover_cache(dir.join("files"), 64 * 1024 * 1024)
                .await?,
        );

        // 预放 Raw 时代的存量:1024² PNG 原图。
        let key = "http://192.0.2.1/big-cover.png";
        let raw_png = png_bytes(/*w*/ 1024, /*h*/ 1024)?;
        cache
            .put_bytes(
                key,
                &raw_png,
                /*subdir*/ "netease",
                &cover_file_name(key, "png"),
            )
            .await?;
        let url = MediaUrl::remote(key)?;
        let cfg = cover_cfg("resized")?;
        // 命中路径不发请求,client 只是签名需要。
        let client = isahc::HttpClient::new().map_err(|e| color_eyre::eyre::eyre!("isahc: {e}"))?;

        // 第一次命中:解码显示照常,后台触发自愈回写。
        let decoded = fetch_and_decode(SourceKind::NETEASE, &url, &client, Some(&cache), &cfg)
            .await
            .ok_or_else(|| color_eyre::eyre::eyre!("命中应解码成功"))?;
        assert!(decoded.clamped, "存量原图首次命中应为 clamped");

        // 等后台编码 + 写盘完成:轮询缓存文件直到字节变小且可解码为 ≤384。
        let path = cache
            .get(key)
            .ok_or_else(|| color_eyre::eyre::eyre!("缓存条目应仍在"))?;
        let mut healed = Vec::<u8>::new();
        for _ in 0..200 {
            let bytes = tokio::fs::read(&path).await?;
            if bytes.len() < raw_png.len()
                && let Ok(img) = image::load_from_memory(&bytes)
                && img.width() <= 384
            {
                healed = bytes;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!healed.is_empty(), "缓存应在时限内被自愈回写为 ≤384 小图");

        // 第二次命中:不再 clamped,也不再回写(字节保持不变)。
        let decoded = fetch_and_decode(SourceKind::NETEASE, &url, &client, Some(&cache), &cfg)
            .await
            .ok_or_else(|| color_eyre::eyre::eyre!("自愈后命中应解码成功"))?;
        assert!(!decoded.clamped, "自愈后不应再 clamped");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let after = tokio::fs::read(&path).await?;
        assert_eq!(after, healed, "二次命中不应再次回写");

        drop(cache);
        drop(std::fs::remove_dir_all(&dir));
        Ok(())
    }

    /// Raw 模式下存量原图命中**不**触发回写(自愈仅 Resized 模式)。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn raw_mode_does_not_rewrite_cache() -> color_eyre::Result<()> {
        let dir = temp_dir();
        std::fs::create_dir_all(&dir)?;
        let store = ClientStore::open(&dir.join("cover.db")).await?;
        let cache = Arc::new(
            store
                .cover_cache(dir.join("files"), 64 * 1024 * 1024)
                .await?,
        );
        let key = "http://192.0.2.1/big-raw.png";
        let raw_png = png_bytes(/*w*/ 1024, /*h*/ 1024)?;
        cache
            .put_bytes(
                key,
                &raw_png,
                /*subdir*/ "netease",
                &cover_file_name(key, "png"),
            )
            .await?;
        let url = MediaUrl::remote(key)?;
        let cfg = cover_cfg("raw")?;
        let client = isahc::HttpClient::new().map_err(|e| color_eyre::eyre::eyre!("isahc: {e}"))?;

        let decoded = fetch_and_decode(SourceKind::NETEASE, &url, &client, Some(&cache), &cfg)
            .await
            .ok_or_else(|| color_eyre::eyre::eyre!("命中应解码成功"))?;
        assert!(decoded.clamped);
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let path = cache
            .get(key)
            .ok_or_else(|| color_eyre::eyre::eyre!("缓存条目应仍在"))?;
        let after = tokio::fs::read(&path).await?;
        assert_eq!(after, raw_png, "Raw 模式不应改写缓存");

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

    /// 缩放地址失败时回退原始 URL,仍拿到字节。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn download_preferring_falls_back_to_canonical() -> color_eyre::Result<()> {
        let bad = mineral_test::mock::serve_once_status(/*status*/ 404, Vec::new()).await?;
        let good = mineral_test::mock::serve_once(b"cover-bytes".to_vec()).await?;
        let client = isahc::HttpClient::new().map_err(|e| color_eyre::eyre::eyre!("isahc: {e}"))?;
        let bytes = download_preferring(&client, Some(bad.to_string()), good.as_str()).await?;
        assert_eq!(bytes, b"cover-bytes", "应回退到原始 URL 的字节");
        Ok(())
    }
}

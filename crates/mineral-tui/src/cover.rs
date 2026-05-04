//! Client 端的封面图 fetcher。
//!
//! 4 个 tokio worker 共享一条 mpsc 队列,跑「裸 HTTP fetch + decode + resize」。
//! 跟 mineral-task 的 lane 不同,本 fetcher **归 client 所有** —— 封面是装饰性
//! 资源,server 不该管。多 client 各持一个 fetcher,各 fetch 各 cache。
//!
//! 设计取舍:
//! - **不做 cancel**:用户切走时积压 fetch 仍跑完,结果进 cache 放着;下次显示直接命中。
//!   减一条复杂度,跟现在(server 端 cancel 后的 cache 命中行为)对齐。
//! - **不做内部 dedup**:dedup 由 caller(`prefetch::ensure_cover` 用 `state.cover_pending`
//!   集合)做。fetcher 单纯 FIFO worker pool。
//! - **错误静默**:fetch / decode 失败只打日志,不推 result,UI 自然显示 fallback。

use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::eyre;
use image::DynamicImage;
use isahc::AsyncReadResponseExt;
use isahc::HttpClient;
use isahc::config::Configurable;
use mineral_model::MediaUrl;
use parking_lot::Mutex;
use tokio::sync::mpsc;

/// 单一 worker 池的并发度。封面图都是几十 KB,4 路够覆盖快速翻 selection。
const WORKERS: usize = 4;

/// HTTP 客户端 timeout。封面比 audio 流小得多,30s 足够慢网兜底。
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// 解码后 resize 到此最大边(像素),保持比例。
///
/// 终端 cell 典型 8×16 px,cover 面板大概 30 cols × 15 rows ≈ 240×240 px;
/// 384 远超显示需求,留点余量给高 DPI / 大字号。原图常常 1024×1024(网易裸 URL),
/// resize 到 384 内存降 ~7x,RGBA 256KB/张 vs 1.5MB+。视觉上完全无损。
const COVER_MAX_DIM: u32 = 384;

/// 就绪 buffer 类型别名。worker 端 push、client tick 端 drain。
type ReadyBuf = Arc<Mutex<Vec<(MediaUrl, Arc<DynamicImage>)>>>;

/// Client 端封面 fetcher。`spawn` 起 4 worker;`request` 投递;`drain_ready` 拉就绪。
pub struct CoverFetcher {
    /// 待 fetch 的 URL 队列。worker 从此抢占式拉。
    req_tx: mpsc::UnboundedSender<MediaUrl>,

    /// worker 完成后塞结果的 buffer;client tick `drain_ready()` 一次拿走。
    ready: ReadyBuf,
}

impl CoverFetcher {
    /// 起 4 worker。caller 必须在 tokio runtime 里(`mineral_tui::run` 是 async fn,
    /// 自然满足);失败通常意味着 isahc 客户端建不起来(系统证书 / TLS 问题等)。
    pub fn spawn() -> color_eyre::Result<Self> {
        let (tx, rx) = mpsc::unbounded_channel::<MediaUrl>();
        let client = HttpClient::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .map_err(|e| eyre!("isahc client init failed: {e}"))?;
        let ready = Arc::new(Mutex::new(Vec::<(MediaUrl, Arc<DynamicImage>)>::new()));
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..WORKERS {
            let rx = Arc::clone(&rx);
            let ready = Arc::clone(&ready);
            let client = client.clone();
            tokio::spawn(async move {
                worker_loop(rx, ready, client).await;
            });
        }
        Ok(Self { req_tx: tx, ready })
    }

    /// 投递一次 fetch 请求。
    pub fn request(&self, url: MediaUrl) {
        // worker 全退出时这里 send 失败,忽略 —— 不该发生(tokio task 跟 fetcher
        // 同生命周期),但即使发生也只是「这张图不显示」,不致命。
        let _ = self.req_tx.send(url);
    }

    /// 把就绪的图拿走。client 主循环 tick 调一次。
    pub fn drain_ready(&self) -> Vec<(MediaUrl, Arc<DynamicImage>)> {
        std::mem::take(&mut *self.ready.lock())
    }
}

async fn worker_loop(
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<MediaUrl>>>,
    ready: ReadyBuf,
    client: HttpClient,
) {
    loop {
        let url = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(u) => u,
                None => return, // 队列关了
            }
        };
        if let Some(img) = fetch_and_decode(&url, &client).await {
            ready.lock().push((url, Arc::new(img)));
        }
    }
}

async fn fetch_and_decode(url: &MediaUrl, client: &HttpClient) -> Option<DynamicImage> {
    let bytes = match read_bytes(url, client).await {
        Ok(b) => b,
        Err(e) => {
            mineral_log::warn!(target: "cover", url = %url, "fetch: {e}");
            return None;
        }
    };
    let img = match image::load_from_memory(&bytes) {
        Ok(i) => i,
        Err(e) => {
            mineral_log::warn!(target: "cover", url = %url, "decode: {e}");
            return None;
        }
    };
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
    Some(resized)
}

async fn read_bytes(url: &MediaUrl, client: &HttpClient) -> color_eyre::Result<Vec<u8>> {
    match url {
        MediaUrl::Remote(u) => {
            let mut resp = client
                .get_async(u.as_str())
                .await
                .map_err(|e| eyre!("http: {e}"))?;
            let bytes = resp.bytes().await.map_err(|e| eyre!("read body: {e}"))?;
            Ok(bytes)
        }
        MediaUrl::Local(p) => tokio::fs::read(p)
            .await
            .map_err(|e| eyre!("read file: {e}")),
    }
}

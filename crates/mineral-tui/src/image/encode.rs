//! Client 端终端图片编码器。
//!
//! 渲染线程只提交缓存未命中的请求；worker 在 blocking pool 中为 Kitty 准备原图
//! shared memory，或为其他协议完成目标尺寸缩放与编码。主循环再按 terminal backend
//! generation 接收成品；编码期间渲染路径继续使用 halfblock，不等待后台任务。

use std::sync::Arc;

use image::DynamicImage;
use parking_lot::Mutex;
use ratatui::layout::Rect;
use tokio::sync::mpsc;

use super::graphics::TerminalBackend;
use super::key::TerminalImageKey;
use super::terminal::TerminalImage;

/// 一次终端图片编码请求。
pub(crate) struct EncodeRequest {
    /// 结果回填终端图片缓存的完整键。
    pub key: TerminalImageKey,

    /// 提交任务时的 terminal backend generation。
    pub generation: u64,

    /// 待编码的完整解码图；仅 rasterized 协议在 worker 内缩放。
    pub image: Arc<DynamicImage>,

    /// 目标 cell 区域；协议 placement 与 halfblocks 网格使用其宽高。
    pub target: Rect,
}

/// 一次编码结果：终端图片成品、像素键与 backend generation。
pub(crate) struct EncodeResult {
    /// 对应请求的终端图片缓存键。
    pub key: TerminalImageKey,

    /// 产生该结果的 terminal backend generation。
    pub generation: u64,

    /// 编码好的终端图片成品，渲染线程只需 place。
    pub terminal_image: TerminalImage,

    /// 成品的缓存预算记账值。
    pub bytes: u64,
}

/// 就绪 buffer:worker 端 push、主循环 `drain_ready` 端取走。
type ReadyBuf = Arc<Mutex<Vec<EncodeResult>>>;

/// Client 端封面编码器。`spawn` 起 worker、`request` 投递、`drain_ready` 收成品。
pub(crate) struct CoverEncoder {
    /// 编码请求队列发送端。
    req_tx: mpsc::UnboundedSender<EncodeRequest>,

    /// worker 完成后塞结果的 buffer;主循环 `drain_ready()` 一次拿走。
    ready: ReadyBuf,
}

impl CoverEncoder {
    /// 起 `workers` 个编码 worker。caller 必须在 tokio runtime 里；worker 与图片引擎
    /// 共享 terminal backend，请求只用 generation 绑定提交时的 backend 生命周期。
    ///
    /// # Params:
    ///   - `workers`: worker 数(配置 `cover.encode_workers`)
    ///   - `backend`: 图片引擎拥有的 terminal backend
    pub(crate) fn spawn(workers: usize, backend: &TerminalBackend) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<EncodeRequest>();
        let ready = Arc::new(Mutex::new(Vec::<EncodeResult>::new()));
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..workers.max(1) {
            let rx = Arc::clone(&rx);
            let ready = Arc::clone(&ready);
            let backend = backend.clone();
            tokio::spawn(async move {
                worker_loop(rx, ready, backend).await;
            });
        }
        Self { req_tx: tx, ready }
    }

    /// 禁用态编码器:不起 worker,纯 null object。`request()` 投递石沉大海、`drain_ready()`
    /// 恒空。**不需要 tokio runtime**,供测试零依赖构造 `App`。
    ///
    /// 仅测试用:生产路径 [`Self::spawn`] 不会失败(只 `tokio::spawn` worker),无降级需要。
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel::<EncodeRequest>();
        Self {
            req_tx: tx,
            ready: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 向 worker 投递一次终端图片编码请求；worker 已退出时静默丢弃。
    pub(crate) fn request(&self, request: EncodeRequest) {
        let _ = self.req_tx.send(request);
    }

    /// 取走就绪终端图片。主循环 tick 调一次并装回 `images.terminal_images`。
    pub(crate) fn drain_ready(&self) -> Vec<EncodeResult> {
        std::mem::take(&mut *self.ready.lock())
    }
}

/// 编码 worker 主循环:从队列拉请求 → `spawn_blocking` 编码 → push 到 ready buffer。
async fn worker_loop(
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<EncodeRequest>>>,
    ready: ReadyBuf,
    backend: TerminalBackend,
) {
    loop {
        let req = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(r) => r,
                None => return, // 队列关了
            }
        };
        if let Some(result) = encode_blocking(req, &backend).await {
            ready.lock().push(result);
        }
    }
}

/// 在 blocking pool 把一张图编码成当前 backend 的终端图片。
///
/// # Params:
///   - `req`: 编码请求(图片身份 + generation + 图 + 目标尺寸)
///   - `backend`: 图片引擎与 worker 共享的当前 terminal backend
///
/// # Return:
///   就绪结果;`spawn_blocking` join 失败返回 `None`(已打日志)。
async fn encode_blocking(req: EncodeRequest, backend: &TerminalBackend) -> Option<EncodeResult> {
    let EncodeRequest {
        key,
        generation,
        image,
        target,
    } = req;
    let graphics = backend.graphics_for(generation)?;
    let pixels = key.pixels();
    let encoded = tokio::task::spawn_blocking(move || {
        let terminal_image =
            TerminalImage::encode(&image, pixels, (target.width, target.height), &graphics)?;
        let bytes = terminal_image.resident_bytes();
        color_eyre::Result::<_>::Ok((terminal_image, bytes))
    })
    .await;
    match encoded {
        Ok(Ok((terminal_image, bytes))) => Some(EncodeResult {
            key,
            generation,
            terminal_image,
            bytes,
        }),
        Ok(Err(error)) => {
            mineral_log::warn!(
                target: "cover",
                error = mineral_log::chain(&error),
                "封面终端协议编码失败"
            );
            None
        }
        Err(e) => {
            let e = color_eyre::Report::new(e);
            mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "封面编码 task join 失败");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_config::CoverProtocolMode;
    use ratatui::layout::Rect;

    use super::{CoverEncoder, EncodeRequest};
    use crate::image::graphics::TerminalGraphics;
    use crate::image::key::{ImageIdentity, PixelSize, TerminalImageKey};
    use crate::image::terminal::TerminalImage;
    use mineral_model::MediaUrl;
    use std::sync::Arc;
    use std::time::Duration;

    /// 编码 worker 产出终端成品并保留请求键与 generation。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn encodes_and_drains() -> color_eyre::Result<()> {
        let graphics = TerminalGraphics::fixed((8, 16));
        let backend =
            crate::image::graphics::TerminalBackend::new(graphics, CoverProtocolMode::Halfblocks);
        let encoder = CoverEncoder::spawn(/*workers*/ 2, &backend);
        let generation = backend.generation();

        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        let image = Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(
            64, 64,
        )));
        let target = Rect::new(0, 0, 20, 10);
        let key = TerminalImageKey::rasterized(
            ImageIdentity::Url(url.clone()),
            PixelSize::from_cells((target.width, target.height), (8, 16)),
        );
        encoder.request(EncodeRequest {
            key: key.clone(),
            generation,
            image,
            target,
        });

        // worker 在另一线程编码,轮询 drain 直到就绪(上限 1s 兜底,正常几十 ms 内)。
        let mut got = Vec::new();
        for _ in 0..100 {
            got = encoder.drain_ready();
            if !got.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        assert_eq!(got.len(), 1, "应收到恰好一个就绪编码结果");
        if let Some(r) = got.first() {
            assert_eq!(r.key, key, "结果键应与请求一致");
            assert_eq!(r.generation, generation, "结果应携带提交时的 generation");
            assert!(
                matches!(&r.terminal_image, TerminalImage::Halfblocks(_)),
                "Halfblocks backend 必须产出对应终端图片"
            );
            assert_eq!(
                r.bytes,
                20 * 10 * 2 * 3,
                "只计半块 RGB 成品，不计原图和已释放的缩放缓冲"
            );
        }
        Ok(())
    }

    /// 两张实际故障尺寸的 Kitty 封面应能同时装入用户的 128 MiB 协议缓存。
    #[test]
    fn two_3000px_kitty_covers_fit_128_mib() -> color_eyre::Result<()> {
        use crate::image::cache::TerminalImageCache;
        use crate::image::graphics::TerminalRelay;
        use crate::image::kitty::KittyImage;

        let cache = TerminalImageCache::new(/*budget*/ 128 * 1024 * 1024);
        let graphics = TerminalGraphics::fixed((8, 16));
        let source = image::DynamicImage::ImageRgb8(image::RgbImage::new(3000, 3000));
        let mut keys = Vec::new();
        for title in ["swiss-army-romance", "places-you-have-come-to-fear"] {
            let key = TerminalImageKey::source(ImageIdentity::Url(MediaUrl::remote(&format!(
                "https://example.com/{title}.jpg"
            ))?));
            let image = TerminalImage::Kitty(KittyImage::encode(
                &source,
                graphics.allocate_kitty_image_id(),
                TerminalRelay::Direct,
            )?);
            let bytes = image.resident_bytes();
            assert!(
                (36_000_000..37_000_000).contains(&bytes),
                "RGBA payload 加少量控制序列，应约 34.3 MiB"
            );
            cache.insert(&key, image, bytes);
            keys.push(key);
        }
        for key in keys {
            assert!(cache.ready(&key), "两张图必须同时留驻，不能互相逐出");
        }
        Ok(())
    }
}

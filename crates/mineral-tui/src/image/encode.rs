//! TUI 端终端图片编码器。
//!
//! 渲染线程只提交缓存未命中的请求；worker 在 blocking pool 中完成格边适配与编码。
//! worker 使用启动时固定的图片协议，请求携带目标尺寸与格边策略。Kitty 原样模式复用
//! 原图，行内封面使用低清像素；编码期间继续显示 halfblock，不等待后台任务。

use std::sync::Arc;

use image::DynamicImage;
use mineral_common::revision::Revision;
use parking_lot::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinError;

use super::graphics::TerminalGraphics;
use super::key::TerminalImageKey;
use super::prepared_image::{self, PreparedImage};

/// 编码实际读取的内容版本；两个缓存的版本号具有独立的计数空间。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum EncodeInput {
    Decoded(Revision),

    Preview(Revision),
}

/// 内容与采样目标共同标识一次编码，旧内容的完成事件不能结束新内容的请求。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct EncodeIdentity {
    pub key: TerminalImageKey,

    pub input: EncodeInput,
}

/// 一次终端图片编码请求。
pub(crate) struct EncodeRequest {
    /// 本次编码读取的内容版本与目标采样几何。
    pub identity: EncodeIdentity,

    /// 待编码的缓存像素；行内缩略图与 rasterized 协议在 worker 内缩放。
    pub image: Arc<DynamicImage>,
}

/// 每个已提交请求都有完成事件，包括入队失败和 blocking task 失败。
pub(crate) struct EncodeCompletion {
    pub identity: EncodeIdentity,

    pub result: Result<PreparedImage, EncodeError>,
}

/// 一次编码任务从入队到成品生成期间的失败原因。
#[derive(Debug, thiserror::Error)]
pub(crate) enum EncodeError {
    #[error("prepare terminal image")]
    Prepare(#[from] prepared_image::Error),

    #[error("join image encoding task")]
    Join(#[from] JoinError),

    #[error("image encoder request queue is closed")]
    QueueClosed,
}

/// 就绪 buffer:worker 端 push、主循环 `drain_ready` 端取走。
type ReadyBuf = Arc<Mutex<Vec<EncodeCompletion>>>;

/// TUI 端封面编码器。`spawn` 起 worker、`request` 投递、`drain_ready` 收成品。
pub(crate) struct CoverEncoder {
    /// 编码请求队列发送端。
    req_tx: mpsc::UnboundedSender<EncodeRequest>,

    /// worker 完成后塞结果的 buffer;主循环 `drain_ready()` 一次拿走。
    ready: ReadyBuf,
}

impl CoverEncoder {
    /// 起 `workers` 个编码 worker。caller 必须在 tokio runtime 里；
    /// worker 与图片引擎共享启动时固定的输出能力。
    ///
    /// # Params:
    ///   - `workers`: worker 数(配置 `cover.encode_workers`)
    ///   - `graphics`: 本次会话的图片协议与 Kitty 资源分配能力
    pub(crate) fn spawn(workers: usize, graphics: &Arc<TerminalGraphics>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<EncodeRequest>();
        let ready = Arc::new(Mutex::new(Vec::<EncodeCompletion>::new()));
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..workers.max(1) {
            let rx = Arc::clone(&rx);
            let ready = Arc::clone(&ready);
            let graphics = Arc::clone(graphics);
            tokio::spawn(async move {
                worker_loop(rx, ready, graphics).await;
            });
        }
        Self { req_tx: tx, ready }
    }

    /// 测试用编码器，不启动 worker；投递后产生队列关闭的失败事件。
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel::<EncodeRequest>();
        Self {
            req_tx: tx,
            ready: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 投递编码请求；队列关闭也发布完成事件，供引擎结束在飞状态。
    pub(crate) fn request(&self, request: EncodeRequest) {
        if let Err(error) = self.req_tx.send(request) {
            self.ready.lock().push(EncodeCompletion {
                identity: error.0.identity,
                result: Err(EncodeError::QueueClosed),
            });
        }
    }

    /// 取走已完成请求；主循环结束在飞状态并安装仍有效的成功成品。
    pub(crate) fn drain_ready(&self) -> Vec<EncodeCompletion> {
        std::mem::take(&mut *self.ready.lock())
    }
}

/// 编码 worker 主循环:从队列拉请求 → `spawn_blocking` 编码 → push 到 ready buffer。
async fn worker_loop(
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<EncodeRequest>>>,
    ready: ReadyBuf,
    graphics: Arc<TerminalGraphics>,
) {
    loop {
        let req = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(r) => r,
                None => return, // 队列关了
            }
        };
        let completion = encode_blocking(req, &graphics).await;
        ready.lock().push(completion);
    }
}

/// 在 blocking pool 按本次会话的协议生成终端图片。
///
/// # Params:
///   - `req`: 编码请求(图片身份、像素源与目标尺寸)
///   - `graphics`: 本次会话固定的输出能力
async fn encode_blocking(req: EncodeRequest, graphics: &Arc<TerminalGraphics>) -> EncodeCompletion {
    let EncodeRequest { identity, image } = req;
    let graphics = Arc::clone(graphics);
    let key = identity.key.clone();
    let result =
        tokio::task::spawn_blocking(move || PreparedImage::encode(&image, &key, &graphics))
            .await
            .map_err(EncodeError::Join)
            .and_then(|result| result.map_err(EncodeError::Prepare));
    EncodeCompletion { identity, result }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::{CoverEncoder, EncodeIdentity, EncodeInput, EncodeRequest};
    use crate::image::cache::ImageCache;
    use crate::image::graphics::TerminalGraphics;
    use crate::image::key::{ImageIdentity, PixelSize, TerminalImageKey};
    use crate::image::kitty::KittyImage;
    use crate::image::prepared_image::PreparedImage;
    use mineral_common::revision::Revision;
    use mineral_model::MediaUrl;
    use std::sync::Arc;
    use std::time::Duration;

    /// 编码 worker 产出终端成品并保留请求键。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn encodes_and_drains() -> color_eyre::Result<()> {
        let graphics = Arc::new(TerminalGraphics::fixed());
        let encoder = CoverEncoder::spawn(/*workers*/ 2, &graphics);

        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        let image = Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(
            64, 64,
        )));
        let target = Rect::new(0, 0, 20, 10);
        let key = TerminalImageKey::rasterized(
            ImageIdentity::Url(url.clone()),
            PixelSize::from_cells((target.width, target.height), (8, 16)),
            (target.width, target.height),
            crate::config::CoverCellFit::Contain,
        );
        encoder.request(EncodeRequest {
            identity: EncodeIdentity {
                key: key.clone(),
                input: EncodeInput::Decoded(Revision::default().next()),
            },
            image,
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
        if let Some(r) = got.into_iter().next() {
            assert_eq!(r.identity.key, key, "结果键应与请求一致");
            let image = r.result?;
            assert!(
                matches!(&image, PreparedImage::Halfblock(_)),
                "Halfblocks backend 必须产出对应终端图片"
            );
            assert_eq!(
                image.resident_bytes(),
                20 * 10 * 2 * 4,
                "只计半块 RGBA 成品，不计原图和已释放的缩放缓冲"
            );
        }
        Ok(())
    }

    /// 两张实际故障尺寸的 Kitty 封面应能同时装入用户的 128 MiB 协议缓存。
    #[test]
    fn two_3000px_kitty_covers_fit_128_mib() -> color_eyre::Result<()> {
        let mut cache = ImageCache::new(/*budget*/ 128 * 1024 * 1024);
        let graphics = TerminalGraphics::fixed_kitty();
        let source = image::DynamicImage::ImageRgb8(image::RgbImage::new(3000, 3000));
        let mut keys = Vec::new();
        for title in ["swiss-army-romance", "places-you-have-come-to-fear"] {
            let key = TerminalImageKey::source(ImageIdentity::Url(MediaUrl::remote(&format!(
                "https://example.com/{title}.jpg"
            ))?));
            let image = PreparedImage::Kitty(KittyImage::encode(&source, &graphics)?);
            let bytes = image.resident_bytes();
            assert!(
                (27_000_000..28_000_000).contains(&bytes),
                "RGB payload 加少量控制序列，应约 25.7 MiB"
            );
            cache.insert(&key, image, bytes);
            keys.push(key);
        }
        for key in keys {
            assert!(cache.contains(&key), "两张图必须同时留驻，不能互相逐出");
        }
        Ok(())
    }
}

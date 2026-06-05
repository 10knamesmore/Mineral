//! Client 端封面 **kitty 协议编码器**:把 resize + base64 编码挪出渲染线程。
//!
//! `cover_image` 渲染封面时,真正昂贵的不是显示而是 **把已解码图 resize 到目标尺寸再
//! base64 编码成 kitty transmit 序列**(一张 384² 图几十毫秒)。原先这步同步跑在渲染线程
//! 上,切歌 / 关浮层触发重编码时会卡掉一帧。这里把它下沉到 worker:
//!
//! - 渲染线程命中已编码协议就直接 place(便宜);未命中则**投递一次编码请求**并先画占位,
//!   不阻塞。
//! - worker 在 `spawn_blocking` 上做 resize + 编码(CPU 密集,别占 runtime worker),完成
//!   塞进 ready buffer。
//! - 主循环 tick `drain_ready()` 把就绪协议装回 `cover_protocols`,之后帧才上真图。
//!
//! 与 [`crate::runtime::cover_fetch`] 同构(request → worker → ready → drain),互补成
//! 「拉取 + 解码」与「resize + 编码」两段都离线的完整异步封面管线。去重由 caller(渲染处的
//! `cover_encode_pending` 集合)做;错误静默(编码失败这张图不显示,UI 留占位)。

use std::sync::Arc;

use image::DynamicImage;
use mineral_model::MediaUrl;
use parking_lot::Mutex;
use ratatui::layout::Rect;
use ratatui_image::picker::Picker;
use ratatui_image::protocol::StatefulProtocol;
use ratatui_image::{Resize, ResizeEncodeRender};
use tokio::sync::mpsc;

/// 一次封面编码请求:把 `image` 编码成适配 `target` 区域的 kitty 协议。
pub struct EncodeRequest {
    /// 封面 URL —— 结果回填 `cover_protocols` 的 key。
    pub url: MediaUrl,

    /// 待编码的已解码图(来自 `cover_cache`,worker 内 resize)。
    pub image: Arc<DynamicImage>,

    /// 目标渲染区域。决定编码尺寸;`(width, height)` 同时作维度键。
    pub target: Rect,
}

/// 一次编码结果:就绪协议 + 维度键,供 tick 装回缓存。
pub struct EncodeResult {
    /// 对应请求的封面 URL。
    pub url: MediaUrl,

    /// 编码所用维度(`target` 的 `(width, height)`),作 `cover_protocols` 维度键。
    pub dims: (u16, u16),

    /// 编码好的有状态协议(渲染线程只需 place,不再重编码)。
    pub protocol: StatefulProtocol,
}

/// 就绪 buffer:worker 端 push、主循环 `drain_ready` 端取走。
type ReadyBuf = Arc<Mutex<Vec<EncodeResult>>>;

/// Client 端封面编码器。`spawn` 起 worker、`sender` 给渲染处投递、`drain_ready` 收成品。
pub struct CoverEncoder {
    /// 编码请求队列发送端;`sender()` 克隆给渲染处投递。
    req_tx: mpsc::UnboundedSender<EncodeRequest>,

    /// worker 完成后塞结果的 buffer;主循环 `drain_ready()` 一次拿走。
    ready: ReadyBuf,
}

impl CoverEncoder {
    /// 起 `workers` 个编码 worker。caller 必须在 tokio runtime 里(`run_app` 跑在
    /// runtime 线程上,满足);`picker` 决定终端图片协议(kitty / sixel / halfblocks)。
    ///
    /// # Params:
    ///   - `picker`: 终端图片协议能力
    ///   - `workers`: worker 数(配置 `cover.encode_workers`)
    pub fn spawn(picker: &Picker, workers: usize) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<EncodeRequest>();
        let ready = Arc::new(Mutex::new(Vec::<EncodeResult>::new()));
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..workers.max(1) {
            let rx = Arc::clone(&rx);
            let ready = Arc::clone(&ready);
            let picker = picker.clone();
            tokio::spawn(async move {
                worker_loop(rx, ready, picker).await;
            });
        }
        Self { req_tx: tx, ready }
    }

    /// 禁用态编码器:不起 worker,纯 null object。`sender()` 投递石沉大海、`drain_ready()`
    /// 恒空。**不需要 tokio runtime**,供测试零依赖构造 `App`。
    ///
    /// 仅测试用:生产路径 [`Self::spawn`] 不会失败(只 `tokio::spawn` worker),无降级需要。
    #[cfg(test)]
    pub fn disabled() -> Self {
        let (tx, _rx) = mpsc::unbounded_channel::<EncodeRequest>();
        Self {
            req_tx: tx,
            ready: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 取一个请求发送端的克隆,交给渲染处投递编码请求。
    pub fn sender(&self) -> mpsc::UnboundedSender<EncodeRequest> {
        self.req_tx.clone()
    }

    /// 把就绪协议拿走。主循环 tick 调一次,装回 `cover_protocols`。
    pub fn drain_ready(&self) -> Vec<EncodeResult> {
        std::mem::take(&mut *self.ready.lock())
    }
}

/// 编码 worker 主循环:从队列拉请求 → `spawn_blocking` 编码 → push 到 ready buffer。
async fn worker_loop(
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<EncodeRequest>>>,
    ready: ReadyBuf,
    picker: Picker,
) {
    loop {
        let req = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(r) => r,
                None => return, // 队列关了
            }
        };
        if let Some(result) = encode_blocking(&picker, req).await {
            ready.lock().push(result);
        }
    }
}

/// 在 blocking 池把一张图编码成适配 `target` 的 kitty 协议。
///
/// resize + base64 编码是 CPU 密集活儿,经 [`tokio::task::spawn_blocking`] 落 blocking 池,
/// 不占 runtime worker。worker 内建好协议后,渲染线程拿同一 `target` 渲染时
/// `needs_resize` 返回 `None`,只 place 不再重编码。
///
/// # Params:
///   - `picker`: 终端图片协议能力(决定 `new_resize_protocol` 产出哪种协议)
///   - `req`: 编码请求(URL + 图 + 目标区域)
///
/// # Return:
///   就绪结果;`spawn_blocking` join 失败返回 `None`(已打日志)。
async fn encode_blocking(picker: &Picker, req: EncodeRequest) -> Option<EncodeResult> {
    let EncodeRequest { url, image, target } = req;
    let dims = (target.width, target.height);
    let picker = picker.clone();
    let encoded = tokio::task::spawn_blocking(move || {
        let mut proto = picker.new_resize_protocol((*image).clone());
        // 与渲染处一致:Scale 模式 + Triangle 滤镜。target 为视觉正方,源亦方图,不变形。
        let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
        if let Some(rect) = proto.needs_resize(&resize, target) {
            proto.resize_encode(&resize, rect);
        }
        proto
    })
    .await;
    match encoded {
        Ok(protocol) => Some(EncodeResult {
            url,
            dims,
            protocol,
        }),
        Err(e) => {
            let e = color_eyre::Report::new(e);
            mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "封面编码 task join 失败");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;
    use ratatui_image::picker::Picker;

    use super::{CoverEncoder, EncodeRequest};
    use mineral_model::MediaUrl;
    use std::sync::Arc;
    use std::time::Duration;

    /// 端到端:投递一次编码请求 → worker 在 blocking 池编码 → `drain_ready` 拿到就绪协议,
    /// url / dims 对得上。halfblocks picker 即可(不依赖真实终端探测);worker 在另一线程跑,
    /// 故需 multi_thread runtime + 轮询等待。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn encodes_and_drains() -> color_eyre::Result<()> {
        let picker = Picker::from_fontsize((8, 16));
        let encoder = CoverEncoder::spawn(&picker, /*workers*/ 2);

        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        let image = Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(
            64, 64,
        )));
        let target = Rect::new(0, 0, 20, 10);
        let _ = encoder.sender().send(EncodeRequest {
            url: url.clone(),
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
            assert_eq!(r.url, url, "结果 url 应与请求一致");
            assert_eq!(r.dims, (20, 10), "结果 dims 应为 target 的 (w,h)");
        }
        Ok(())
    }
}

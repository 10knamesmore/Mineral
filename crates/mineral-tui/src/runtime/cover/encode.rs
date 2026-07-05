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
//! - 主循环 tick `drain_ready()` 把就绪协议装回 `covers.protocols`,之后帧才上真图。
//!
//! 与 [`crate::runtime::cover::fetch`] 同构(request → worker → ready → drain),互补成
//! 「拉取 + 解码」与「resize + 编码」两段都离线的完整异步封面管线。去重由 caller(渲染处的
//! `covers.encode_pending` 集合)做;错误静默(编码失败这张图不显示,UI 留占位)。

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
    /// 封面 URL —— 结果回填 `covers.protocols` 的 key。
    pub url: MediaUrl,

    /// 待编码的已解码图(来自 `covers.cache`,worker 内 resize)。
    pub image: Arc<DynamicImage>,

    /// 目标渲染区域。决定编码尺寸;`(width, height)` 同时作维度键。
    pub target: Rect,

    /// 编码用的终端协议能力 + 当前 cell 字号。**逐请求携带**而非 worker 自持:字号是
    /// 编码尺寸换算的分母,终端字号变化后必须用新值,否则封面按旧 cell 比例铺 —— 偏小占
    /// 一小块 / 偏大被裁。渲染处投递时塞入当前 `App.picker`,worker 用它不留 stale 副本。
    pub picker: Picker,
}

/// 一次编码结果:就绪协议 + 维度键,供 tick 装回缓存。
pub struct EncodeResult {
    /// 对应请求的封面 URL。
    pub url: MediaUrl,

    /// 编码所用维度(`target` 的 `(width, height)`),作 `covers.protocols` 维度键。
    pub dims: (u16, u16),

    /// 编码好的有状态协议(渲染线程只需 place,不再重编码)。
    pub protocol: StatefulProtocol,

    /// 该协议估算的常驻字节数(源图副本 + 目标编码序列),供 `ProtocolCache` 字节预算记账。
    /// ratatui-image 不暴露协议内部大小,故按"源像素 + 目标像素 × RGBA × base64"估。
    pub bytes: u64,
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
    /// runtime 线程上,满足)。终端图片协议能力随每个 [`EncodeRequest`] 携带(`picker`
    /// 字段),worker 不自持,故字号变化后编码即用新 picker。
    ///
    /// # Params:
    ///   - `workers`: worker 数(配置 `cover.encode_workers`)
    pub fn spawn(workers: usize) -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<EncodeRequest>();
        let ready = Arc::new(Mutex::new(Vec::<EncodeResult>::new()));
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        for _ in 0..workers.max(1) {
            let rx = Arc::clone(&rx);
            let ready = Arc::clone(&ready);
            tokio::spawn(async move {
                worker_loop(rx, ready).await;
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

    /// 把就绪协议拿走。主循环 tick 调一次,装回 `covers.protocols`。
    pub fn drain_ready(&self) -> Vec<EncodeResult> {
        std::mem::take(&mut *self.ready.lock())
    }
}

/// 编码 worker 主循环:从队列拉请求 → `spawn_blocking` 编码 → push 到 ready buffer。
async fn worker_loop(
    rx: Arc<tokio::sync::Mutex<mpsc::UnboundedReceiver<EncodeRequest>>>,
    ready: ReadyBuf,
) {
    loop {
        let req = {
            let mut g = rx.lock().await;
            match g.recv().await {
                Some(r) => r,
                None => return, // 队列关了
            }
        };
        if let Some(result) = encode_blocking(req).await {
            ready.lock().push(result);
        }
    }
}

/// 在 blocking 池把一张图编码成适配 `target` 的 kitty 协议。
///
/// resize + base64 编码是 CPU 密集活儿,经 [`tokio::task::spawn_blocking`] 落 blocking 池,
/// 不占 runtime worker。worker 内建好协议后,渲染线程拿同一 `target` 渲染时
/// `needs_resize` 返回 `None`,只 place 不再重编码。协议能力取自 `req.picker`(逐请求携带,
/// 保证字号变化后用新值)。
///
/// # Params:
///   - `req`: 编码请求(URL + 图 + 目标区域 + picker)
///
/// # Return:
///   就绪结果;`spawn_blocking` join 失败返回 `None`(已打日志)。
async fn encode_blocking(req: EncodeRequest) -> Option<EncodeResult> {
    let EncodeRequest {
        url,
        image,
        target,
        picker,
    } = req;
    let dims = (target.width, target.height);
    let font = picker.font_size();
    let encoded = tokio::task::spawn_blocking(move || {
        // 源图副本常驻在协议里(ImageSource 留原图供尺寸变化时重 resize),记进字节账。
        let source_bytes = u64::try_from(image.as_bytes().len()).unwrap_or(u64::MAX);
        let mut proto = picker.new_resize_protocol((*image).clone());
        // 与渲染处一致:Scale 模式 + Triangle 滤镜。target 为视觉正方,源亦方图,不变形。
        let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
        if let Some(rect) = proto.needs_resize(&resize, target) {
            proto.resize_encode(&resize, rect);
        }
        (proto, source_bytes)
    })
    .await;
    match encoded {
        Ok((protocol, source_bytes)) => Some(EncodeResult {
            url,
            dims,
            protocol,
            bytes: source_bytes.saturating_add(encoded_bytes_estimate(target, font)),
        }),
        Err(e) => {
            let e = color_eyre::Report::new(e);
            mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "封面编码 task join 失败");
            None
        }
    }
}

/// 估算目标编码序列字节数:目标像素 × RGBA(4) × base64(4/3)。
///
/// ratatui-image 不暴露协议编码缓冲大小,这里按量级估(不求精确,只需与真实占用同阶、
/// 单调),供 `ProtocolCache` 字节预算排序 / 封顶。
///
/// # Params:
///   - `target`: 目标区域(cell 数)
///   - `font`: 单 cell 像素尺寸 `(w, h)`
///
/// # Return:
///   估算字节数
fn encoded_bytes_estimate(target: Rect, font: (u16, u16)) -> u64 {
    let px_w = u64::from(target.width).saturating_mul(u64::from(font.0));
    let px_h = u64::from(target.height).saturating_mul(u64::from(font.1));
    let pixels = px_w.saturating_mul(px_h);
    pixels.saturating_mul(4).saturating_mul(4) / 3
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
        let encoder = CoverEncoder::spawn(/*workers*/ 2);

        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        let image = Arc::new(image::DynamicImage::ImageRgba8(image::RgbaImage::new(
            64, 64,
        )));
        let target = Rect::new(0, 0, 20, 10);
        let _ = encoder.sender().send(EncodeRequest {
            url: url.clone(),
            image,
            target,
            picker,
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

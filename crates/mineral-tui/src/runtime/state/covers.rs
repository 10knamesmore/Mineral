//! 封面管线的 client 端状态:原图/色板缓存、在飞集合、已编码协议。
//!
//! fetch(下载 + 解码 + 取色)与 encode(resize + 终端协议编码)两个 worker
//! 的结果都在这里落地;渲染处只读缓存、未命中时投递请求。

use std::cell::RefCell;
use std::sync::Arc;

use image::DynamicImage;
use mineral_model::MediaUrl;
use ratatui_image::protocol::StatefulProtocol;
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::sync::mpsc;

use crate::render::palette::CoverPalette;
use crate::runtime::cover::encode::{CoverEncoder, EncodeRequest};
use crate::runtime::cover::fetch::CoverFetcher;

/// 一条 cover protocol 缓存项:`(协议, 上次渲染时的目标 cells dims)`。
///
/// dims 用于 invalidation —— 跟当前 area 不一致就重建 protocol,避免字号 / 终端
/// 大小变了之后图按旧 dims 绘出来溢出 / 截断。
pub type CoverProtocolEntry = (StatefulProtocol, (u16, u16));

/// 封面管线状态([`AppState`](crate::runtime::state::AppState) 的封面域)。
pub struct CoverHub {
    /// 已拉好的封面原始图(URL → 解码后的 RGB 像素)。session 内一直留。
    pub cache: FxHashMap<MediaUrl, Arc<DynamicImage>>,

    /// 已取色的封面色板(URL → 频谱 2D 色场的重点色,Lab 明度升序)。
    /// 缺 key = 没取到色(取色失败 / 还没回传)。session 内一直留,顺手缓存复用。
    pub palettes: FxHashMap<MediaUrl, CoverPalette>,

    /// 上次已应用到频谱的封面 key(频谱当前色场对应哪张封面)。
    /// `None` = 频谱在 hue 漂移(无封面 / 取色未就绪)。`sync_spectrum_palette` 身份判定用。
    pub spectrum_cover: Option<MediaUrl>,

    /// 在飞 fetch 集合,用于 dedup tick 重复请求。
    pub pending: FxHashSet<MediaUrl>,

    /// 渲染用的 ratatui-image stateful protocol 缓存。`StatefulProtocol` 内部记编码状态
    /// (kitty 的图片 id、sixel 编码缓冲等),render 复用就不会每帧重发图。
    /// 用 `RefCell` 是因为 `view::draw` 拿 `&AppState`,而 stateful_widget 渲染要 `&mut`。
    pub protocols: RefCell<FxHashMap<MediaUrl, CoverProtocolEntry>>,

    /// 封面编码请求发送端(投递给 [`CoverEncoder`] 的 worker)。
    /// 渲染处未命中已编码协议时投一次,把 resize + base64 编码挪出渲染线程。禁用态(测试 /
    /// 无 runtime)是个无接收端的 sender,投递静默丢弃。
    pub encode_tx: mpsc::UnboundedSender<EncodeRequest>,

    /// 在飞编码 `(URL, 维度)` 集合,渲染处据此 dedup —— 同一封面同尺寸只投一次,等结果回填。
    /// 用 `RefCell` 因渲染拿 `&AppState`。
    pub encode_pending: RefCell<FxHashSet<(MediaUrl, (u16, u16))>>,

    /// 当前 client-side cover_fetcher in-flight 数(等价 `pending.len()`,
    /// 每 tick 由 App 灌入)。
    pub loading: usize,
}

impl CoverHub {
    /// 构造空 hub。`encode_tx` 默认是无接收端的 sender(投递即丢);真实 worker
    /// 由 `App::new` 注入 `CoverEncoder::sender()` 覆盖。
    pub(crate) fn new() -> Self {
        Self {
            cache: FxHashMap::default(),
            palettes: FxHashMap::default(),
            spectrum_cover: None,
            pending: FxHashSet::default(),
            protocols: RefCell::new(FxHashMap::default()),
            encode_tx: mpsc::unbounded_channel().0,
            encode_pending: RefCell::new(FxHashSet::default()),
            loading: 0,
        }
    }

    /// 把 fetch worker 就绪的图写进 `cache` + 色板写进 `palettes` + 清掉对应 protocol
    /// (下次渲染重建)。取色失败(`palette = None`)只是不落色板,图照常缓存显示。
    pub(crate) fn drain_ready_covers(&mut self, fetcher: &CoverFetcher) {
        for ready in fetcher.drain_ready() {
            self.pending.remove(&ready.url);
            if let Some(palette) = ready.palette {
                self.palettes.insert(ready.url.clone(), palette);
            }
            self.cache.insert(ready.url.clone(), ready.image);
            self.protocols.borrow_mut().remove(&ready.url);
        }
    }

    /// 把编码 worker 就绪的封面协议装回 `protocols`,并出 `encode_pending`。
    /// 之后帧渲染该封面即命中已编码协议、直接 place,不再在渲染线程上 resize / 编码。
    pub(crate) fn drain_ready_protocols(&mut self, encoder: &CoverEncoder) {
        for r in encoder.drain_ready() {
            self.encode_pending
                .borrow_mut()
                .remove(&(r.url.clone(), r.dims));
            self.protocols
                .borrow_mut()
                .insert(r.url, (r.protocol, r.dims));
        }
    }
}

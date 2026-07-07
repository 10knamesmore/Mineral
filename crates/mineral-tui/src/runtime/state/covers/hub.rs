//! 封面管线的 client 端状态:原图/色板缓存、在飞集合、已编码协议。
//!
//! fetch(下载 + 解码 + 取色)与 encode(resize + 终端协议编码)两个 worker
//! 的结果都在这里落地;渲染处只读缓存、未命中时投递请求。

use std::cell::RefCell;
use std::sync::Arc;

use image::DynamicImage;
use mineral_model::MediaUrl;
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::sync::mpsc;

use super::cache::CoverCache;
use super::protocols::ProtocolCache;
use crate::render::palette::CoverPalette;
use crate::runtime::cover::encode::{CoverEncoder, EncodeRequest};
use crate::runtime::cover::fetch::CoverFetcher;

/// 封面管线状态([`AppState`](crate::runtime::state::AppState) 的封面域)。
pub struct CoverHub {
    /// 已拉好的封面原始图(字节预算 LRU;越 `tui.cover.cache.image` 逐出最久未用)。
    /// 逐出项派生的协议 / 色板由 `drain_ready_covers` 联动清理,不留悬挂。
    pub cache: CoverCache,

    /// 已取色的封面色板(URL → 频谱 2D 色场的重点色,Lab 明度升序)。
    /// 缺 key = 没取到色(取色失败 / 还没回传)。session 内一直留,顺手缓存复用。
    pub palettes: FxHashMap<MediaUrl, CoverPalette>,

    /// 上次已应用到频谱的封面 key(频谱当前色场对应哪张封面)。
    /// `None` = 频谱在 hue 漂移(无封面 / 取色未就绪)。`sync_spectrum_palette` 身份判定用。
    pub spectrum_cover: Option<MediaUrl>,

    /// 在飞 fetch 集合,用于 dedup tick 重复请求。
    pub pending: FxHashSet<MediaUrl>,

    /// 已编码封面协议缓存(字节预算 LRU;越 `tui.cover.cache.protocol` 逐出最久未渲染)。
    /// `StatefulProtocol` 内部记编码状态(kitty 图片 id、sixel 缓冲)+ 源图副本,render 复用
    /// 就不每帧重编;逐出的滚回时后台重编、其间 halfblock 兜底。
    pub protocols: ProtocolCache,

    /// 封面编码请求发送端(投递给 [`CoverEncoder`] 的 worker)。
    /// 渲染处未命中已编码协议时投一次,把 resize + base64 编码挪出渲染线程。禁用态(测试 /
    /// 无 runtime)是个无接收端的 sender,投递静默丢弃。
    pub encode_tx: mpsc::UnboundedSender<EncodeRequest>,

    /// 在飞编码 `(URL, 维度)` 集合,渲染处据此 dedup —— 同一封面同尺寸只投一次,等结果回填。
    /// 用 `RefCell` 因渲染拿 `&AppState`。
    pub encode_pending: RefCell<FxHashSet<(MediaUrl, (u16, u16))>>,

    /// 歌单拼贴合成键 → 上次合成时的就绪成员数。渐进式重拼判定:成员图逐张到货,
    /// 就绪数超过记录值才重拼覆盖同 key(成员 fetch 失败静默、`pending` 不回收,
    /// 等不来"全员就绪",只能有几张拼几张)。成员集变化即换新键,旧记录仅占位无害。
    pub(crate) collage_ready: FxHashMap<MediaUrl, usize>,

    /// 当前 client-side cover_fetcher in-flight 数(等价 `pending.len()`,
    /// 每 tick 由 App 灌入)。
    pub loading: usize,
}

impl CoverHub {
    /// 构造空 hub。`encode_tx` 默认是无接收端的 sender(投递即丢);真实 worker
    /// 由 `App::new` 注入 `CoverEncoder::sender()` 覆盖。
    ///
    /// # Params:
    ///   - `image_budget`: 封面原图缓存的字节预算(配置 `tui.cover.cache.image`)
    ///   - `protocol_budget`: 已编码协议缓存的字节预算(配置 `tui.cover.cache.protocol`)
    pub(crate) fn new(image_budget: u64, protocol_budget: u64) -> Self {
        Self {
            cache: CoverCache::new(image_budget),
            palettes: FxHashMap::default(),
            spectrum_cover: None,
            pending: FxHashSet::default(),
            protocols: ProtocolCache::new(protocol_budget),
            encode_tx: mpsc::unbounded_channel().0,
            encode_pending: RefCell::new(FxHashSet::default()),
            collage_ready: FxHashMap::default(),
            loading: 0,
        }
    }

    /// 现调两层缓存预算(配置热更):缩小立即逐出直到回落、**不清缓存**;
    /// 原图侧被逐出项的派生物(协议 / 色板 / 频谱标记)照常联动清理。
    ///
    /// # Params:
    ///   - `image_budget`: 原图缓存新预算(配置 `tui.cover.cache.image`)
    ///   - `protocol_budget`: 协议缓存新预算(配置 `tui.cover.cache.protocol`)
    pub(crate) fn set_budgets(&mut self, image_budget: u64, protocol_budget: u64) {
        let evicted = self.cache.set_budget(image_budget);
        for url in evicted {
            self.discard_derived(&url);
        }
        self.protocols.set_budget(protocol_budget);
    }

    /// 塞入一张本地合成图(歌单拼贴),与 fetch 回填同规则:清掉该 key 旧协议(下次渲染
    /// 按新图重建),被逐出项的派生物联动清理。
    pub(crate) fn insert_synthesized(&mut self, url: &MediaUrl, image: Arc<DynamicImage>) {
        let evicted = self.cache.insert(url, image);
        self.protocols.remove(url);
        for u in evicted {
            self.discard_derived(&u);
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
            let evicted = self.cache.insert(&ready.url, ready.image);
            self.protocols.remove(&ready.url);
            for url in evicted {
                self.discard_derived(&url);
            }
        }
    }

    /// 清掉某封面 URL 派生的一切:已编码协议、取色色板;若它正是频谱当前色场来源,
    /// 一并解除标记让频谱下 tick 回退 hue。原图已被 LRU 逐出,这些派生物再留即悬挂。
    fn discard_derived(&mut self, url: &MediaUrl) {
        self.protocols.remove(url);
        self.palettes.remove(url);
        if self.spectrum_cover.as_ref() == Some(url) {
            self.spectrum_cover = None;
        }
    }

    /// 把编码 worker 就绪的封面协议装回 `protocols`,并出 `encode_pending`。
    /// 之后帧渲染该封面即命中已编码协议、直接 place,不再在渲染线程上 resize / 编码。
    pub(crate) fn drain_ready_protocols(&mut self, encoder: &CoverEncoder) {
        for r in encoder.drain_ready() {
            self.encode_pending
                .borrow_mut()
                .remove(&(r.url.clone(), r.dims));
            self.protocols.insert(&r.url, r.dims, r.protocol, r.bytes);
        }
    }
}

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
use crate::render::anim::Transition;
use crate::render::palette::CoverPalette;
use crate::runtime::cover::encode::{CoverEncoder, EncodeRequest, EncodeResult};
use crate::runtime::cover::fetch::CoverFetcher;
use crate::runtime::cover::kitty_transmit::{self, TransmitBacklog, TransmitBatch};

/// 一段进行中的全屏切歌封面转场:新旧两图按样式逐帧合成 halfblock,推满落定回
/// 终端图协议高清。转场窗口恰好盖住新图的离线编码期,落定无占位闪。
pub struct CoverTransition {
    /// 退场封面(切歌前封面区显示的图)。
    pub from_url: MediaUrl,

    /// 进场封面(在播新图)。
    pub to_url: MediaUrl,

    /// 转场进度(进场方向,推满即落定;时长 = `cover_transition.duration_ms`)。
    pub anim: Transition,
}

/// 封面管线状态([`AppState`](crate::runtime::state::AppState) 的封面域)。
pub struct CoverHub {
    /// 已拉好的封面原始图(字节预算 LRU;越 `tui.cover.cache.image` 逐出最久未用)。
    /// 逐出项派生的协议 / 色板由 `drain_ready_covers` 联动清理,不留悬挂。
    pub cache: CoverCache,

    /// 已取色的封面色板(URL → 频谱 2D 色场的重点色,Lab 明度升序)。
    /// 缺 key = 没取到色(取色失败 / 还没回传)。session 内一直留,顺手缓存复用。
    pub palettes: FxHashMap<MediaUrl, CoverPalette>,

    /// 上次已应用到频谱的封面 key(频谱当前色场对应哪张封面)。
    /// `None` = 频谱在 hue 漂移(无封面 / 取色未就绪)。`sync_cover_palette` 身份判定用。
    pub spectrum_cover: Option<MediaUrl>,

    /// 当前播放封面的色板拷贝(频谱 / 波形共用的稳定源),与 `spectrum_cover` 同处维护。
    /// **刻意不每帧读 `palettes`**:那是原图 LRU 的派生物,browse 滚动 churn 会把在播曲
    /// 的色板逐出又重取,直接读它会让已播段渐变在 Gradient↔Solid 间闪烁;持一份拷贝
    /// 只随封面**身份变化**更新,对逐出免疫。`None` = 取色失败 / 无封面(回落单色)。
    pub current_palette: Option<CoverPalette>,

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
    /// 流式传输开启时,key 保留到图数据**送达终端**才出集(编码 + 传输合并为一段在飞期,
    /// 防止传输途中渲染未命中又重投编码)。用 `RefCell` 因渲染拿 `&AppState`。
    pub encode_pending: RefCell<FxHashSet<(MediaUrl, (u16, u16))>>,

    /// kitty 图数据流式传输 backlog:编码就绪时取出的 transmit 序列在此排队,
    /// 主循环每帧按预算写给终端;传完才放行对应协议槽的渲染命中。
    kitty_backlog: TransmitBacklog,

    /// 进行中的全屏切歌封面转场;`None` = 稳态(命中协议直接 place 高清)。
    /// 触发 / 推进 / 收尾都在 app 层的转场同步,渲染处只读。
    pub transition: Option<CoverTransition>,

    /// 全屏封面区当前实际显示的封面(转场 from 的身份依据)。与在播封面 diff 出
    /// 切歌瞬间;非全屏稳态时只跟随不触发。
    pub displayed_cover: Option<MediaUrl>,

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
            current_palette: None,
            pending: FxHashSet::default(),
            protocols: ProtocolCache::new(protocol_budget),
            encode_tx: mpsc::unbounded_channel().0,
            encode_pending: RefCell::new(FxHashSet::default()),
            kitty_backlog: TransmitBacklog::default(),
            transition: None,
            displayed_cover: None,
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

    /// 把编码 worker 就绪的封面协议装回 `protocols`。之后帧渲染该封面即命中已编码
    /// 协议、直接 place,不再在渲染线程上 resize / 编码。
    ///
    /// # Params:
    ///   - `sizes_per_image`: 同一封面并存的已编码尺寸槽上限(调用方现读配置传入)
    ///   - `stream_transmit`: 是否启用 kitty 图数据流式传输(现读配置传入)
    pub(crate) fn drain_ready_protocols(
        &mut self,
        encoder: &CoverEncoder,
        sizes_per_image: usize,
        stream_transmit: bool,
    ) {
        for r in encoder.drain_ready() {
            self.install_protocol(r, sizes_per_image, stream_transmit);
        }
    }

    /// 装一条编码结果:流式传输开启且是 kitty 时,先把 transmit 序列取进 backlog、
    /// 槽标「待传输」、`encode_pending` 保留到传完;否则(关闭 / 非 kitty)按原行为
    /// 立即出集、槽即刻可命中(图数据在首次 place 那帧整段发送)。
    fn install_protocol(&mut self, mut r: EncodeResult, sizes_per_image: usize, stream: bool) {
        let payload = if stream {
            kitty_transmit::extract_transmit(&mut r.protocol)
        } else {
            None
        };
        let awaiting = payload.is_some();
        if let Some(payload) = payload {
            self.kitty_backlog.push(r.url.clone(), r.dims, payload);
        } else {
            self.encode_pending
                .borrow_mut()
                .remove(&(r.url.clone(), r.dims));
        }
        self.protocols.insert(
            &r.url,
            r.dims,
            r.protocol,
            r.bytes,
            sizes_per_image,
            awaiting,
        );
    }

    /// 按字节预算弹一批待写终端的 kitty 图数据;空 backlog 返回 `None`。
    ///
    /// 调用方写达终端后**必须**调 [`Self::finish_transmitted`] 应用批里的完成键,
    /// 顺序不可反——占位符绝不先于图数据 place。
    pub(crate) fn drain_transmit(&mut self, budget_bytes: usize) -> Option<TransmitBatch> {
        self.kitty_backlog.drain_budget(budget_bytes)
    }

    /// 应用一批已送达终端的传输完成键:解除协议槽「待传输」、`encode_pending` 出集。
    /// 槽已被逐出 / 清空则空转。
    pub(crate) fn finish_transmitted(&mut self, completed: &[(MediaUrl, (u16, u16))]) {
        for (url, dims) in completed {
            self.encode_pending
                .borrow_mut()
                .remove(&(url.clone(), *dims));
            self.protocols.mark_transmit_done(url, *dims);
        }
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use image::{DynamicImage, RgbImage};
    use mineral_model::MediaUrl;
    use ratatui::layout::Rect;
    use ratatui_image::picker::{Picker, ProtocolType};
    use ratatui_image::{Resize, ResizeEncodeRender};

    use super::CoverHub;
    use crate::runtime::cover::encode::EncodeResult;

    /// 造一条已编码好的结果;`kind` 决定协议形态(kitty 才有可提取的 transmit)。
    fn encode_result(url: &MediaUrl, kind: ProtocolType) -> EncodeResult {
        let mut picker = Picker::from_fontsize((8, 16));
        picker.set_protocol_type(kind);
        let image = DynamicImage::ImageRgb8(RgbImage::new(32, 32));
        let mut protocol = picker.new_resize_protocol(image);
        let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
        let target = Rect::new(0, 0, 8, 4);
        if let Some(rect) = protocol.needs_resize(&resize, target) {
            protocol.resize_encode(&resize, rect);
        }
        EncodeResult {
            url: url.clone(),
            dims: (target.width, target.height),
            protocol,
            bytes: 100,
        }
    }

    /// 流式开启 + kitty:装入即「待传输」——不参与渲染命中、`encode_pending` 保留;
    /// 传输批写完应用完成键后恢复命中、出集。
    #[test]
    fn streamed_kitty_gates_until_transmitted() -> color_eyre::Result<()> {
        let mut hub = CoverHub::new(
            /*image_budget*/ 1 << 20,
            /*protocol_budget*/ 1 << 20,
        );
        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        let key = (url.clone(), (8, 4));
        hub.encode_pending.borrow_mut().insert(key.clone());

        hub.install_protocol(
            encode_result(&url, ProtocolType::Kitty),
            /*sizes_per_image*/ 3,
            /*stream*/ true,
        );

        assert!(
            hub.encode_pending.borrow().contains(&key),
            "传输途中在飞集不出集,防重投编码"
        );
        assert!(
            !hub.protocols.render_hit(&url, (8, 4), |_| {}),
            "传输途中不参与渲染命中"
        );

        // 大预算一批传完 → 写达终端后应用完成键。
        let batch = hub
            .drain_transmit(/*budget_bytes*/ 1 << 20)
            .ok_or_else(|| eyre!("backlog 应有待写批"))?;
        assert!(batch.bytes.starts_with("\x1b_G"), "批内容应是 APC 图数据链");
        assert_eq!(batch.completed, vec![key.clone()], "一批传完回吐完成键");
        hub.finish_transmitted(&batch.completed);

        assert!(
            !hub.encode_pending.borrow().contains(&key),
            "传完在飞集出集"
        );
        assert!(
            hub.protocols.render_hit(&url, (8, 4), |_| {}),
            "传完恢复渲染命中"
        );
        assert!(hub.drain_transmit(1 << 20).is_none(), "backlog 应已排空");
        Ok(())
    }

    /// 流式关闭:kitty 结果按原行为装入——立即出集、立即可命中(首次 place 整段发送)。
    #[test]
    fn stream_disabled_installs_immediately() -> color_eyre::Result<()> {
        let mut hub = CoverHub::new(
            /*image_budget*/ 1 << 20,
            /*protocol_budget*/ 1 << 20,
        );
        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        let key = (url.clone(), (8, 4));
        hub.encode_pending.borrow_mut().insert(key.clone());

        hub.install_protocol(
            encode_result(&url, ProtocolType::Kitty),
            /*sizes_per_image*/ 3,
            /*stream*/ false,
        );

        assert!(!hub.encode_pending.borrow().contains(&key), "装入即出集");
        assert!(
            hub.protocols.render_hit(&url, (8, 4), |_| {}),
            "装入即可命中"
        );
        assert!(hub.drain_transmit(1 << 20).is_none(), "无流式任务");
        Ok(())
    }

    /// 流式开启但非 kitty(halfblocks):无从提取,按原行为立即装入,不进 backlog。
    #[test]
    fn stream_non_kitty_installs_immediately() -> color_eyre::Result<()> {
        let mut hub = CoverHub::new(
            /*image_budget*/ 1 << 20,
            /*protocol_budget*/ 1 << 20,
        );
        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        let key = (url.clone(), (8, 4));
        hub.encode_pending.borrow_mut().insert(key.clone());

        hub.install_protocol(
            encode_result(&url, ProtocolType::Halfblocks),
            /*sizes_per_image*/ 3,
            /*stream*/ true,
        );

        assert!(!hub.encode_pending.borrow().contains(&key), "装入即出集");
        assert!(
            hub.protocols.render_hit(&url, (8, 4), |_| {}),
            "装入即可命中"
        );
        assert!(
            hub.drain_transmit(1 << 20).is_none(),
            "非 kitty 不进 backlog"
        );
        Ok(())
    }
}

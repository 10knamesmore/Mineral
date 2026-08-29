//! 图片管线的 client 端状态：解码图与色板缓存、在飞集合、终端图片成品。
//!
//! preview、按需 decode 与 encode worker 的结果都在这里落地。预取生成低清真实封面；稳定
//! 渲染 miss 登记完整 decode demand，由主循环统一调度。

use std::cell::RefCell;
use std::sync::Arc;

use image::DynamicImage;
use mineral_model::MediaUrl;
use mineral_model::SourceKind;
use ratatui::layout::Rect;
use rustc_hash::{FxHashMap, FxHashSet};

use super::cache::{CoverCache, TerminalImageCache};
use super::graphics::{TerminalBackend, TerminalGraphics};
use super::key::{ImageIdentity, PixelSize, TerminalImageKey};
#[cfg(test)]
use super::terminal::TerminalImage;
use crate::image::encode::{CoverEncoder, EncodeRequest, EncodeResult};
use crate::image::fetch::{CoverCompletion, CoverFetcher, CoverRequestKind};
use crate::render::anim::{Transition, ticks16_from_ms};
use crate::render::palette::CoverPalette;

/// 图片下载与终端成品编码 worker 的进程内句柄。
struct ImageWorkers {
    /// 图片下载、解码与取色 worker。
    fetcher: CoverFetcher,

    /// 终端图片编码 worker。
    encoder: CoverEncoder,
}

/// 一种已在稳定布局中出现的 preview 目标尺寸。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct PreviewTarget {
    /// 目标 cell 宽高。
    cells: (u16, u16),

    /// cell 几何折算出的真实像素尺寸。
    pixels: PixelSize,
}

impl PreviewTarget {
    /// 从目标区域与终端 cell 像素尺寸构造 preview 几何。
    fn from_area(area: Rect, cell_pixels: (u16, u16)) -> Self {
        let cells = (area.width, area.height);
        Self {
            cells,
            pixels: PixelSize::from_cells(cells, cell_pixels),
        }
    }

    /// 为图片身份构造 preview 缓存键。
    fn key(self, identity: ImageIdentity) -> TerminalImageKey {
        TerminalImageKey::rasterized(identity, self.pixels)
    }
}

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

/// 负责图片获取、终端成品缓存、渲染政策与 worker/backend 生命周期的中央引擎。
pub struct ImageEngine {
    /// 与应用状态共享的当前配置，整棵替换后所有现读政策立即生效。
    cfg: Arc<mineral_config::Config>,

    /// 图片引擎与编码 worker 共享的唯一 terminal backend。
    terminal_backend: TerminalBackend,

    /// 已拉好的封面原始图(字节预算 LRU;越 `tui.cover.cache.image` 逐出最久未用)。
    /// 逐出项派生的终端图片与色板由 fetch drain 联动清理，不留悬挂。
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

    /// 正在执行的 preview 或 decode URL；只表达真实 in-flight。
    pub pending: FxHashSet<MediaUrl>,

    /// 已知图片 URL 对应的来源；render miss 据此选择磁盘缓存子目录。
    source_by_url: FxHashMap<MediaUrl, SourceKind>,

    /// 本进程 preview 失败的 URL；与完整 decode 失败分离，稳定显示仍可按需尝试。
    preview_failures: FxHashSet<MediaUrl>,

    /// 上一帧稳定布局观察到的 preview 尺寸；渲染持共享引用，故内部可变。
    observed_preview_targets: RefCell<FxHashSet<PreviewTarget>>,

    /// 当前预取拍使用的 preview 尺寸，由 [`Self::tick`] 从上一帧观察值刷新。
    preview_targets: Vec<PreviewTarget>,

    /// 实际显示或显式 prepare 提出的 decode demand；渲染只持共享引用，故内部可变。
    decode_demand: RefCell<FxHashSet<MediaUrl>>,

    /// 本进程按需 decode 失败的 URL；失败后继续显示 preview，preview 也没有才为空白。
    decode_failures: RefCell<FxHashSet<MediaUrl>>,

    /// 协议无关的真实封面低清 preview 缓存；协议切换和完整 decoded LRU 逐出都不清理。
    pub preview_images: TerminalImageCache,

    /// 终端图片成品缓存(字节预算 LRU;越 `tui.cover.cache.protocol` 逐出最久未渲染)。
    /// 成品保留协议渲染状态与资源，render 命中后无需每帧重编；逐出的图片滚回时
    /// 后台重编，其间使用 halfblock。
    pub terminal_images: TerminalImageCache,

    /// 由图片引擎独占的下载与终端成品编码 worker。
    workers: ImageWorkers,

    /// 在飞终端图片键集合，渲染处据此去重。用 `RefCell` 因渲染拿 `&AppState`。
    pub encode_pending: RefCell<FxHashSet<TerminalImageKey>>,

    /// 进行中的全屏切歌封面转场；`None` 表示稳态。
    /// 触发、推进与收尾都由 [`Self::tick`] 统一处理，渲染处只读。
    pub transition: Option<CoverTransition>,

    /// 全屏封面区当前实际显示的封面(转场 from 的身份依据)。与在播封面 diff 出
    /// 切歌瞬间;非全屏稳态时只跟随不触发。
    pub displayed_cover: Option<MediaUrl>,

    /// 歌单拼贴合成键 → 上次合成时的就绪成员数。成员图逐张到货时渐进式重拼；
    /// decode 失败成员不阻塞已就绪图片合成。
    pub(crate) collage_ready: FxHashMap<MediaUrl, usize>,
}

impl ImageEngine {
    /// 用已启动的 fetcher 与终端探测结果构造完整图片引擎。
    ///
    /// # Params:
    ///   - `cfg`: 与应用状态共享的当前配置
    ///   - `fetcher`: 图片下载、解码与取色 worker
    ///   - `graphics`: 启动期协商出的终端图片能力
    pub(crate) fn new(
        cfg: Arc<mineral_config::Config>,
        fetcher: CoverFetcher,
        graphics: TerminalGraphics,
    ) -> Self {
        let mode = *cfg.tui().cover().protocol();
        let encode_workers = *cfg.tui().cover().encode_workers();
        let terminal_backend = TerminalBackend::new(graphics, mode);
        let encoder = CoverEncoder::spawn(encode_workers, &terminal_backend);
        Self::from_parts(cfg, fetcher, encoder, terminal_backend)
    }

    /// 构造不启动 worker 的测试图片引擎。
    #[cfg(test)]
    pub(crate) fn disabled(cfg: Arc<mineral_config::Config>) -> Self {
        let terminal_backend = TerminalBackend::fixed((8, 16));
        Self::from_parts(
            cfg,
            CoverFetcher::disabled(),
            CoverEncoder::disabled(),
            terminal_backend,
        )
    }

    /// 从已经确定的 worker 与 backend 构造图片引擎状态。
    fn from_parts(
        cfg: Arc<mineral_config::Config>,
        fetcher: CoverFetcher,
        encoder: CoverEncoder,
        terminal_backend: TerminalBackend,
    ) -> Self {
        let image_budget = *cfg.tui().cover().cache().image();
        let preview_budget = *cfg.tui().cover().cache().preview();
        let protocol_budget = *cfg.tui().cover().cache().protocol();
        Self {
            cfg,
            terminal_backend,
            cache: CoverCache::new(image_budget),
            palettes: FxHashMap::default(),
            spectrum_cover: None,
            current_palette: None,
            pending: FxHashSet::default(),
            source_by_url: FxHashMap::default(),
            preview_failures: FxHashSet::default(),
            observed_preview_targets: RefCell::new(FxHashSet::default()),
            preview_targets: Vec::new(),
            decode_demand: RefCell::new(FxHashSet::default()),
            decode_failures: RefCell::new(FxHashSet::default()),
            preview_images: TerminalImageCache::new(preview_budget),
            terminal_images: TerminalImageCache::new(protocol_budget),
            workers: ImageWorkers { fetcher, encoder },
            encode_pending: RefCell::new(FxHashSet::default()),
            transition: None,
            displayed_cover: None,
            collage_ready: FxHashMap::default(),
        }
    }

    /// 向 worker 投递一次终端图片编码请求。
    ///
    /// # Params:
    ///   - `request`: 已解码图片、终端成品键与 backend generation
    pub(crate) fn request_encode(&self, request: EncodeRequest) {
        self.workers.encoder.request(request);
    }

    /// 应用新配置；预算变化保留内容，协议变化替换整个 terminal backend。
    ///
    /// # Params:
    ///   - `cfg`: 新的有效配置
    pub(crate) fn apply_config(&mut self, cfg: Arc<mineral_config::Config>) {
        let image_budget = *cfg.tui().cover().cache().image();
        let preview_budget = *cfg.tui().cover().cache().preview();
        let protocol_budget = *cfg.tui().cover().cache().protocol();
        self.cfg = cfg;
        self.set_budgets(image_budget, preview_budget, protocol_budget);
        self.apply_graphics_mode();
    }

    /// 刷新终端 cell 像素尺寸；尺寸变化由终端成品键自然产生 miss。
    pub(crate) fn refresh_cell_pixels(&mut self) {
        let _ = self.terminal_backend.refresh_cell_pixels();
    }

    /// 返回当前单个终端 cell 的像素宽高。
    pub(crate) fn cell_pixels(&self) -> (u16, u16) {
        self.terminal_backend.cell_pixels()
    }

    /// 返回当前 terminal backend generation。
    pub(crate) fn graphics_generation(&self) -> u64 {
        self.terminal_backend.generation()
    }

    /// 返回 cover transition 的 zoom 缩放倍数。
    pub(crate) fn transition_zoom_scale(&self) -> f32 {
        *self.cfg.tui().cover_transition().zoom().scale()
    }

    /// 返回当前生效的终端图协议。
    pub(crate) fn graphics_protocol(&self) -> crate::image::graphics::GraphicsProtocol {
        self.terminal_backend.protocol()
    }

    /// 插入一条测试用终端图片成品。
    #[cfg(test)]
    pub(crate) fn insert_test_terminal_image(&self, url: &MediaUrl, cells: (u16, u16)) {
        let key = TerminalImageKey::rasterized(
            ImageIdentity::Url(url.clone()),
            crate::image::key::PixelSize::from_cells(cells, self.cell_pixels()),
        );
        self.terminal_images
            .insert(&key, TerminalImage::test_halfblocks(), /*bytes*/ 1);
    }

    /// 插入一条测试用真实低清 preview。
    #[cfg(test)]
    pub(crate) fn insert_test_preview(&self, url: &MediaUrl, cells: (u16, u16)) {
        let key = TerminalImageKey::rasterized(
            ImageIdentity::Url(url.clone()),
            PixelSize::from_cells(cells, self.cell_pixels()),
        );
        self.preview_images
            .insert(&key, TerminalImage::test_halfblocks(), /*bytes*/ 1);
    }

    /// 将配置的协议模式应用到当前终端能力。
    fn apply_graphics_mode(&mut self) {
        let mode = *self.cfg.tui().cover().protocol();
        if self.terminal_backend.apply_mode(mode) {
            self.clear_terminal_state();
        }
    }

    /// 清空 terminal backend 的全部协议相关状态。
    fn clear_terminal_state(&mut self) {
        self.terminal_images.clear();
        self.encode_pending.borrow_mut().clear();
    }

    /// 现调三层 RAM 缓存预算(配置热更):缩小立即逐出直到回落、**不清缓存**;
    /// 原图侧被逐出项的派生物(协议 / 色板 / 频谱标记)照常联动清理。
    ///
    /// # Params:
    ///   - `image_budget`: 原图缓存新预算(配置 `tui.cover.cache.image`)
    ///   - `preview_budget`: preview 缓存新预算(配置 `tui.cover.cache.preview`)
    ///   - `protocol_budget`: 协议缓存新预算(配置 `tui.cover.cache.protocol`)
    pub(crate) fn set_budgets(
        &mut self,
        image_budget: u64,
        preview_budget: u64,
        protocol_budget: u64,
    ) {
        let evicted = self.cache.set_budget(image_budget);
        for url in evicted {
            self.discard_derived(&url);
        }
        self.preview_images.set_budget(preview_budget);
        self.terminal_images.set_budget(protocol_budget);
    }

    /// 塞入一张本地合成图(歌单拼贴),与 fetch 回填同规则:清掉该 key 旧协议(下次渲染
    /// 按新图重建),被逐出项的派生物联动清理。
    pub(crate) fn insert_synthesized(&mut self, url: &MediaUrl, image: Arc<DynamicImage>) {
        let evicted = self.cache.insert(url, image);
        self.terminal_images
            .remove(&ImageIdentity::Url(url.clone()));
        for u in evicted {
            self.discard_derived(&u);
        }
    }

    /// 消费 preview 与 decode completion，并结束对应 in-flight。
    fn drain_cover_completions(&mut self) {
        for completion in self.workers.fetcher.drain_ready() {
            match completion {
                CoverCompletion::Preview(ready) => self.install_preview(ready),
                CoverCompletion::Decoded(ready) => self.install_decoded_cover(ready),
                CoverCompletion::Failed { url, kind } => {
                    self.pending.remove(&url);
                    match kind {
                        CoverRequestKind::Preview => {
                            self.preview_failures.insert(url);
                        }
                        CoverRequestKind::Decode => {
                            self.decode_demand.borrow_mut().remove(&url);
                            self.decode_failures.borrow_mut().insert(url);
                        }
                    }
                }
            }
        }
    }

    /// 把低清真实封面装入独立 preview LRU。
    ///
    /// # Params:
    ///   - `ready`: preview 键、halfblock 成品与字节数
    fn install_preview(&mut self, ready: crate::image::fetch::CoverPreviewReady) {
        self.pending.remove(&ready.url);
        self.preview_failures.remove(&ready.url);
        mineral_log::debug!(
            target: "prefetch",
            url = %ready.url,
            bytes = ready.bytes,
            "cover preview ready"
        );
        self.preview_images
            .insert(&ready.key, ready.image, ready.bytes);
    }

    /// 把按需解码结果写入 RAM LRU，并清理旧终端派生物。
    ///
    /// # Params:
    ///   - `ready`: 解码图与色板
    fn install_decoded_cover(&mut self, ready: crate::image::fetch::CoverReady) {
        self.pending.remove(&ready.url);
        self.decode_demand.borrow_mut().remove(&ready.url);
        self.decode_failures.borrow_mut().remove(&ready.url);
        if let Some(palette) = ready.palette {
            self.palettes.insert(ready.url.clone(), palette);
        }
        let evicted = self.cache.insert(&ready.url, ready.image);
        self.terminal_images
            .remove(&ImageIdentity::Url(ready.url.clone()));
        for url in evicted {
            self.discard_derived(&url);
        }
    }

    /// 清掉某封面 URL 派生的一切：终端图片、取色色板；若它正是频谱当前色场来源，
    /// 一并解除标记让频谱下 tick 回退 hue。原图已被 LRU 逐出,这些派生物再留即悬挂。
    fn discard_derived(&mut self, url: &MediaUrl) {
        self.terminal_images
            .remove(&ImageIdentity::Url(url.clone()));
        self.palettes.remove(url);
        if self.spectrum_cover.as_ref() == Some(url) {
            self.spectrum_cover = None;
        }
    }

    /// 把编码 worker 就绪的终端图片装回缓存；旧 generation 的结果直接丢弃。
    fn drain_ready_terminal_images(&mut self) {
        let ready_images = self.workers.encoder.drain_ready();
        for result in ready_images {
            self.install_terminal_image(result);
        }
    }

    /// 推进图片 worker 回填、终端成品安装与全屏切图转场。
    ///
    /// # Params:
    ///   - `current_cover`: 当前播放图片身份
    ///   - `fullscreen_stable`: 全屏布局是否已经稳定
    pub(crate) fn tick(&mut self, current_cover: Option<MediaUrl>, fullscreen_stable: bool) {
        self.refresh_preview_targets();
        self.drain_cover_completions();
        self.schedule_decode_demand();
        self.drain_ready_terminal_images();
        self.sync_transition(current_cover, fullscreen_stable);
    }

    /// 返回正在执行的图片 preview 与 decode 总数。
    pub(crate) fn loading_count(&self) -> usize {
        self.pending.len()
    }

    /// 把上一帧渲染观察到的 preview 尺寸交给本拍预取，并清空观察集合。
    fn refresh_preview_targets(&mut self) {
        self.preview_targets = self
            .observed_preview_targets
            .get_mut()
            .drain()
            .collect::<Vec<PreviewTarget>>();
        self.preview_targets.sort_by_key(|target| {
            (
                target.cells.0,
                target.cells.1,
                target.pixels.width(),
                target.pixels.height(),
            )
        });
    }

    /// 记录稳定布局实际使用的 preview 尺寸，供下一拍半径预取复用。
    ///
    /// # Params:
    ///   - `area`: 已按终端几何收成正方形的图片区域
    pub(crate) fn observe_preview_target(&self, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.observed_preview_targets
            .borrow_mut()
            .insert(PreviewTarget::from_area(area, self.cell_pixels()));
    }

    /// 为 URL 与目标区域构造 preview 缓存键。
    ///
    /// # Params:
    ///   - `url`: 图片源 URL
    ///   - `area`: preview 的目标 cell 区域
    pub(crate) fn preview_key(&self, url: &MediaUrl, area: Rect) -> TerminalImageKey {
        PreviewTarget::from_area(area, self.cell_pixels()).key(ImageIdentity::Url(url.clone()))
    }

    /// 返回图片与尺寸是否仍需要生成 preview。
    fn should_prepare_preview(&self, url: &MediaUrl, key: &TerminalImageKey) -> bool {
        !self.cache.contains_key(url)
            && !self.preview_images.contains(key)
            && !self.preview_failures.contains(url)
            && !self.pending.contains(url)
            && !self.decode_demand.borrow().contains(url)
    }

    /// 登记一张实际显示或显式 prepare 所需的图片；下次 tick 按需解码。
    ///
    /// # Params:
    ///   - `url`: 需要完整像素的图片 URL
    pub(crate) fn demand_decode(&self, url: &MediaUrl) {
        if self.cache.contains_key(url) || self.decode_failures.borrow().contains(url) {
            return;
        }
        self.decode_demand.borrow_mut().insert(url.clone());
    }

    /// 为非渲染消费者登记来源并立即请求完整解码图。
    ///
    /// # Params:
    ///   - `candidates`: 需要完整像素的来源与 URL
    pub(crate) fn load(&mut self, candidates: impl IntoIterator<Item = (SourceKind, MediaUrl)>) {
        for (source, url) in candidates {
            self.source_by_url.insert(url.clone(), source);
            self.decode_demand.borrow_mut().insert(url.clone());
            self.request_decode(&url);
        }
    }

    /// 去重并为上一帧稳定布局提交真实低清 preview 候选。
    ///
    /// # Params:
    ///   - `candidates`: 按优先顺序排列的来源与 URL
    pub(crate) fn prefetch(
        &mut self,
        candidates: impl IntoIterator<Item = (SourceKind, MediaUrl)>,
    ) {
        let targets = self.preview_targets.clone();
        for (source, url) in candidates {
            self.source_by_url.insert(url.clone(), source);
            if self.cache.contains_key(&url) {
                continue;
            }
            for target in &targets {
                let key = target.key(ImageIdentity::Url(url.clone()));
                if !self.should_prepare_preview(&url, &key) {
                    continue;
                }
                self.pending.insert(url.clone());
                mineral_log::debug!(
                    target: "prefetch",
                    url = %url,
                    ?source,
                    cell_width = target.cells.0,
                    cell_height = target.cells.1,
                    pixel_width = target.pixels.width(),
                    pixel_height = target.pixels.height(),
                    "generate cover preview"
                );
                if !self
                    .workers
                    .fetcher
                    .preview(source, url.clone(), key, target.cells)
                {
                    self.pending.remove(&url);
                    self.preview_failures.insert(url.clone());
                }
                break;
            }
        }
    }

    /// 调度渲染路径累计的 decode demand。
    fn schedule_decode_demand(&mut self) {
        let demanded = self
            .decode_demand
            .borrow()
            .iter()
            .cloned()
            .collect::<Vec<MediaUrl>>();
        for url in demanded {
            self.request_decode(&url);
        }
    }

    /// 在来源已知且没有同 URL in-flight 时提交一次 decode。
    ///
    /// # Params:
    ///   - `url`: 需要完整像素的图片 URL
    fn request_decode(&mut self, url: &MediaUrl) {
        if self.cache.contains_key(url) || self.decode_failures.borrow().contains(url) {
            self.decode_demand.borrow_mut().remove(url);
            return;
        }
        if self.pending.contains(url) {
            return;
        }
        let Some(source) = self.source_by_url.get(url).copied() else {
            return;
        };
        self.pending.insert(url.clone());
        mineral_log::debug!(target: "prefetch", url = %url, ?source, "decode demanded cover");
        if !self.workers.fetcher.decode(source, url.clone()) {
            self.pending.remove(url);
            self.decode_demand.borrow_mut().remove(url);
            self.decode_failures.borrow_mut().insert(url.clone());
        }
    }

    /// 推进或创建全屏切图转场。
    pub(crate) fn sync_transition(
        &mut self,
        current_cover: Option<MediaUrl>,
        fullscreen_stable: bool,
    ) {
        if let Some(active) = self.transition.as_mut() {
            active.anim.tick();
            if active.anim.at_max() {
                self.transition = None;
            }
        }
        if !fullscreen_stable {
            self.displayed_cover = current_cover;
            self.transition = None;
            return;
        }
        if self.displayed_cover == current_cover {
            return;
        }
        let previous = std::mem::replace(&mut self.displayed_cover, current_cover.clone());
        let transition = self.cfg.tui().cover_transition();
        if !*transition.enabled() {
            return;
        }
        let (Some(from_url), Some(to_url)) = (previous, current_cover) else {
            return;
        };
        if !(self.cache.contains_key(&from_url) && self.cache.contains_key(&to_url)) {
            return;
        }
        self.transition = Some(CoverTransition {
            from_url,
            to_url,
            anim: Transition::expanding(ticks16_from_ms(
                *transition.duration_ms(),
                *self.cfg.tui().animation().frame_tick_ms(),
            )),
        });
    }

    /// 装入一条当前 generation 的终端图片成品并解除在飞键。
    fn install_terminal_image(&mut self, result: EncodeResult) {
        if result.generation != self.graphics_generation() {
            return;
        }
        self.encode_pending.borrow_mut().remove(&result.key);
        self.terminal_images
            .insert(&result.key, result.terminal_image, result.bytes);
    }
}

//! 图片管线的 TUI 端状态：解码图与色板缓存、在飞集合、终端图片成品。
//!
//! preview、按需 decode 与 encode worker 的结果都在这里落地。预取生成低清真实封面；稳定
//! 布局准备登记完整 decode demand，由主循环统一调度。

use std::sync::Arc;

use crate::config::CoverCellFit;
use image::{DynamicImage, RgbaImage};
use mineral_model::MediaUrl;
use mineral_model::SourceKind;
use ratatui::layout::Rect;
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::sync::oneshot;

use super::cache::{CoverCache, ImageCache};
use super::graphics::{GraphicsProtocol, TerminalGraphics, window_cell_pixels};
use super::halfblock::HalfblockImage;
#[cfg(test)]
use super::key::PixelSize;
use super::key::{ImageIdentity, PreviewTarget, TerminalImageKey};
use super::prepared_image::PreparedImage;
use crate::image::CoverFingerprint;
use crate::image::encode::{
    CoverEncoder, EncodeCompletion, EncodeError, EncodeIdentity, EncodeInput, EncodeRequest,
};
use crate::image::fetch::{CoverCompletion, CoverFetcher, CoverRequestKind, OriginalImageError};
use crate::image::resize;

/// 图片下载与终端成品编码 worker 的进程内句柄。
struct ImageWorkers {
    /// 图片下载、解码与取色 worker。
    fetcher: CoverFetcher,

    /// 终端图片编码 worker。
    encoder: CoverEncoder,
}

/// 负责图片请求生命周期、终端成品缓存、渲染政策与 worker 的中央引擎。
/// preview 与 decode 候选的缓存、失败和在途判断均在此收敛。
pub struct ImageEngine {
    /// 与应用状态共享的当前配置，整棵替换后所有现读政策立即生效。
    cfg: Arc<crate::config::TuiConfig>,

    /// 图片引擎与编码 worker 共享的本次会话输出能力。
    graphics: Arc<TerminalGraphics>,

    /// 主循环维护的字符像素宽高；编码请求携带提交时的目标尺寸。
    cell_pixels: (u16, u16),

    /// 已拉好的封面原始图(字节预算 LRU;越 `cover.cache.image` 逐出最久未用)。
    /// 逐出时联动清理高清终端成品与色板；行内缩略图保留自己的小像素资源。
    pub cache: CoverCache,

    /// 正在执行的 preview 或 decode URL；只表达真实 in-flight。
    pending: FxHashSet<MediaUrl>,

    /// 已知图片 URL 对应的来源；准备入口据此选择磁盘缓存子目录。
    source_by_url: FxHashMap<MediaUrl, SourceKind>,

    /// 本进程 preview 失败的 URL；与完整 decode 失败分离，稳定显示仍可按需尝试。
    preview_failures: FxHashSet<MediaUrl>,

    /// 本次稳定布局声明的 preview 尺寸。
    observed_preview_targets: FxHashSet<PreviewTarget>,

    /// 当前预取拍使用的 preview 尺寸，在准备结束时从本次布局需求刷新。
    preview_targets: Vec<PreviewTarget>,

    /// 布局准备或显式预热提出的完整解码需求。
    decode_demand: FxHashSet<MediaUrl>,

    /// 本进程按需 decode 失败的 URL；失败后继续显示 preview，preview 也没有才为空白。
    decode_failures: FxHashSet<MediaUrl>,

    /// 真实封面低清 preview 缓存；完整 decoded LRU 逐出时不清理。
    pub preview_images: ImageCache<HalfblockImage>,

    /// 终端图片成品缓存(字节预算 LRU;越 `cover.cache.protocol` 逐出最久未渲染)。
    /// 成品保留协议渲染状态与资源，render 命中后无需每帧重编；逐出的图片滚回时
    /// 后台重编，其间使用 halfblock。
    pub terminal_images: ImageCache<PreparedImage>,

    /// 本次准备要求的行内图片 placement；输出提交时生成协议指令。
    pub(super) graphics_placements: Vec<(TerminalImageKey, ratatui::layout::Rect)>,

    /// 由图片引擎独占的下载与终端成品编码 worker。
    workers: ImageWorkers,

    /// 真实在飞的内容版本与采样目标；相同 URL 的旧、新内容可以同时在飞。
    pub(crate) encode_pending: FxHashSet<EncodeIdentity>,

    /// 同一内容与目标失败后不再提交；新内容或新目标不受旧失败影响。
    encode_failures: FxHashMap<EncodeIdentity, EncodeError>,
}

impl ImageEngine {
    /// 用已启动的 fetcher 与终端探测结果构造完整图片引擎。
    ///
    /// # Params:
    ///   - `cfg`: 与应用状态共享的当前配置
    ///   - `fetcher`: 图片下载、解码与取色 worker
    ///   - `graphics`: 启动期协商出的终端图片能力
    ///   - `cell_pixels`: 启动时查询的字符像素宽高
    pub(crate) fn new(
        cfg: Arc<crate::config::TuiConfig>,
        fetcher: CoverFetcher,
        graphics: TerminalGraphics,
        cell_pixels: (u16, u16),
    ) -> Self {
        let encode_workers = *cfg.cover().encode_workers();
        let graphics = Arc::new(graphics);
        let encoder = CoverEncoder::spawn(encode_workers, &graphics);
        Self::from_parts(cfg, fetcher, encoder, graphics, cell_pixels)
    }

    /// 构造不启动 worker 的测试图片引擎。
    #[cfg(test)]
    pub(crate) fn disabled(cfg: Arc<crate::config::TuiConfig>) -> Self {
        Self::from_parts(
            cfg,
            CoverFetcher::disabled(),
            CoverEncoder::disabled(),
            Arc::new(TerminalGraphics::fixed()),
            (8, 16),
        )
    }

    /// 构造支持 Kitty 的测试引擎；不启动 worker，也不探测真实终端。
    #[cfg(test)]
    pub(crate) fn disabled_kitty(cfg: Arc<crate::config::TuiConfig>) -> Self {
        Self::from_parts(
            cfg,
            CoverFetcher::disabled(),
            CoverEncoder::disabled(),
            Arc::new(TerminalGraphics::fixed_kitty()),
            (8, 16),
        )
    }

    /// 插入一张真实 Kitty 缩略图，供列表布局与终端输出测试使用。
    #[cfg(test)]
    pub(crate) fn insert_test_thumbnail(&mut self, url: &MediaUrl) -> color_eyre::Result<()> {
        let key = self.thumbnail_preview_key(url);
        let source = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            32,
            32,
            image::Rgb([180, 80, 40]),
        ));
        let image = PreparedImage::encode(&source, &key, &self.graphics)?;
        let bytes = image.resident_bytes();
        self.terminal_images.insert(&key, image, bytes);
        Ok(())
    }

    /// 从已经确定的 worker、会话输出能力与显示尺寸构造图片引擎。
    fn from_parts(
        cfg: Arc<crate::config::TuiConfig>,
        fetcher: CoverFetcher,
        encoder: CoverEncoder,
        graphics: Arc<TerminalGraphics>,
        cell_pixels: (u16, u16),
    ) -> Self {
        let image_budget = *cfg.cover().cache().image();
        let preview_budget = *cfg.cover().cache().preview();
        let protocol_budget = *cfg.cover().cache().protocol();
        Self {
            cfg,
            graphics,
            cell_pixels,
            cache: CoverCache::new(image_budget),
            pending: FxHashSet::default(),
            source_by_url: FxHashMap::default(),
            preview_failures: FxHashSet::default(),
            observed_preview_targets: FxHashSet::default(),
            preview_targets: Vec::new(),
            decode_demand: FxHashSet::default(),
            decode_failures: FxHashSet::default(),
            preview_images: ImageCache::new(preview_budget),
            terminal_images: ImageCache::new(protocol_budget),
            graphics_placements: Vec::new(),
            workers: ImageWorkers { fetcher, encoder },
            encode_pending: FxHashSet::default(),
            encode_failures: FxHashMap::default(),
        }
    }

    /// 从源文件缓存或网络加载原尺寸图片，结果独立于显示缓存和终端渲染。
    pub(crate) fn load_original(
        &self,
        source: SourceKind,
        url: MediaUrl,
    ) -> oneshot::Receiver<Result<RgbaImage, OriginalImageError>> {
        self.workers.fetcher.original(source, url)
    }

    /// 向 worker 投递一次终端图片编码请求。
    ///
    /// # Params:
    ///   - `request`: 已解码图片、终端成品键与目标区域
    pub(crate) fn request_encode(&mut self, request: EncodeRequest) {
        let identity = &request.identity;
        if !self.should_encode(identity) {
            return;
        }
        self.encode_pending.insert(identity.clone());
        self.workers.encoder.request(request);
    }

    pub(super) fn should_encode(&self, identity: &EncodeIdentity) -> bool {
        !self.terminal_images.contains(&identity.key)
            && !self.encode_pending.contains(identity)
            && !self.encode_failures.contains_key(identity)
    }

    /// 行内图优先读取独立 preview；高清成品只读取完整解码图。
    pub(super) fn encode_input(&self, key: &TerminalImageKey) -> Option<EncodeInput> {
        if matches!(key, TerminalImageKey::Thumbnail { .. })
            && let Some(revision) = self.preview_images.revision(key)
        {
            return Some(EncodeInput::Preview(revision));
        }
        let ImageIdentity::Url(url) = key.identity();
        self.cache.revision(url).map(EncodeInput::Decoded)
    }

    /// 应用格边适配与缓存预算；图片协议只在启动时选择，修改后需重启。
    ///
    /// # Params:
    ///   - `cfg`: 新的有效配置
    pub(crate) fn apply_config(&mut self, cfg: Arc<crate::config::TuiConfig>) {
        let image_budget = *cfg.cover().cache().image();
        let preview_budget = *cfg.cover().cache().preview();
        let protocol_budget = *cfg.cover().cache().protocol();
        let cell_fit = *cfg.cover().cell_fit();
        if cfg.cover().protocol() != self.cfg.cover().protocol() {
            mineral_log::info!(
                target: "cover",
                requested = ?cfg.cover().protocol(),
                active = ?self.graphics.protocol(),
                "cover.protocol 已变更，重启 TUI 后生效"
            );
        }
        if self.cell_fit() != cell_fit {
            mineral_log::debug!(target: "cover", previous = ?self.cell_fit(), ?cell_fit,
                "cover cell fit changed; decoded images and cached variants retained");
            self.observed_preview_targets = self
                .observed_preview_targets
                .drain()
                .map(|target| target.with_cell_fit(cell_fit))
                .collect();
            for target in &mut self.preview_targets {
                *target = target.with_cell_fit(cell_fit);
            }
        }
        self.cfg = cfg;
        self.set_budgets(image_budget, preview_budget, protocol_budget);
    }

    /// 刷新终端 cell 像素尺寸；尺寸变化由终端成品键自然产生 miss。
    pub(crate) fn refresh_cell_pixels(&mut self) {
        if let Some(cell_pixels) = window_cell_pixels() {
            self.cell_pixels = cell_pixels;
        }
    }

    /// 返回当前单个终端 cell 的像素宽高。
    pub(crate) fn cell_pixels(&self) -> (u16, u16) {
        self.cell_pixels
    }

    /// 现读主封面的格边适配策略；worker 使用请求键中固定的策略。
    pub(super) fn cell_fit(&self) -> CoverCellFit {
        *self.cfg.cover().cell_fit()
    }

    /// 返回当前生效的终端图协议。
    pub(crate) fn graphics_protocol(&self) -> GraphicsProtocol {
        self.graphics.protocol()
    }

    /// 插入一条测试用终端图片成品。
    #[cfg(test)]
    pub(crate) fn insert_test_terminal_image(&mut self, url: &MediaUrl, cells: (u16, u16)) {
        let key = TerminalImageKey::rasterized(
            ImageIdentity::Url(url.clone()),
            PixelSize::from_cells(cells, self.cell_pixels()),
            cells,
            self.cell_fit(),
        );
        self.terminal_images
            .insert(&key, PreparedImage::test_halfblock(), /*bytes*/ 1);
    }

    /// 现调三层 RAM 缓存预算(配置热更):缩小立即逐出直到回落、**不清缓存**;
    /// 原图侧被逐出项的高清成品联动清理，保留行内缩略图。
    ///
    /// # Params:
    ///   - `image_budget`: 原图缓存新预算(配置 `cover.cache.image`)
    ///   - `preview_budget`: preview 缓存新预算(配置 `cover.cache.preview`)
    ///   - `protocol_budget`: 协议缓存新预算(配置 `cover.cache.protocol`)
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
    /// 按新图重建),被逐出项的派生物联动清理。拼贴图小、且只在重拼时合成一次,
    /// 指纹就地算。
    pub(crate) fn insert_synthesized(
        &mut self,
        url: &MediaUrl,
        image: Arc<DynamicImage>,
    ) -> Result<(), resize::Error> {
        let fingerprint = CoverFingerprint::of(&image)?;
        let evicted = self.cache.insert(url, image, fingerprint, None);
        let identity = ImageIdentity::Url(url.clone());
        // 合成图覆盖内容时旧 preview 也失效；普通 decoded 逐出仍保留独立缩略图。
        self.preview_images.remove(&identity);
        self.terminal_images.remove(&identity);
        for u in evicted {
            self.discard_derived(&u);
        }
        Ok(())
    }

    /// 消费 preview 与 decode completion，并结束对应 in-flight。
    fn drain_cover_completions(&mut self) {
        for completion in self.workers.fetcher.drain_ready() {
            self.apply_completion(completion);
        }
    }

    /// 应用一条图片 worker 完成事件:安装 preview / 显示图，并结束对应 in-flight。
    fn apply_completion(&mut self, completion: CoverCompletion) {
        match completion {
            CoverCompletion::Preview(ready) => self.install_preview(ready),
            CoverCompletion::PreviewAndDecoded { preview, full } => {
                // 非 JPEG 源一次解码的双产物:preview 先显示,显示图进入 RAM LRU。
                self.install_preview(preview);
                self.install_decoded_cover(full);
            }
            CoverCompletion::Decoded(ready) => self.install_decoded_cover(ready),
            CoverCompletion::Failed { url, kind } => {
                self.pending.remove(&url);
                match kind {
                    CoverRequestKind::Preview => {
                        self.preview_failures.insert(url);
                    }
                    CoverRequestKind::Decode => {
                        mineral_log::warn!(
                            target: "cover_cache",
                            url = %url,
                            "封面解码失败:该 URL 不再重试"
                        );
                        self.decode_demand.remove(&url);
                        self.decode_failures.insert(url);
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
        if matches!(ready.key, TerminalImageKey::Thumbnail { .. }) {
            self.terminal_images.remove_key(&ready.key);
        }
        self.preview_images
            .insert(&ready.key, ready.image, ready.bytes);
    }

    /// 把按需解码结果写入 RAM LRU，并清理旧终端派生物。
    ///
    /// # Params:
    ///   - `ready`: 解码图与色板
    fn install_decoded_cover(&mut self, ready: crate::image::fetch::CoverReady) {
        self.pending.remove(&ready.url);
        self.decode_demand.remove(&ready.url);
        self.decode_failures.remove(&ready.url);
        let evicted = self
            .cache
            .insert(&ready.url, ready.image, ready.fingerprint, ready.palette);
        self.terminal_images
            .remove_decoded(&ImageIdentity::Url(ready.url.clone()));
        for url in evicted {
            self.discard_derived(&url);
        }
    }

    /// 原图逐出后清理高清成品，保留拥有独立小像素资源的行内封面。
    fn discard_derived(&mut self, url: &MediaUrl) {
        self.terminal_images
            .remove_decoded(&ImageIdentity::Url(url.clone()));
    }

    /// 把编码 worker 就绪的终端图片装回缓存。
    fn drain_ready_terminal_images(&mut self) {
        let ready_images = self.workers.encoder.drain_ready();
        for result in ready_images {
            self.install_terminal_image(result);
        }
    }

    /// 消费图片 worker 完成事件并提交待解码需求。
    pub(crate) fn tick(&mut self) {
        self.drain_cover_completions();
        self.schedule_decode_demand();
        self.drain_ready_terminal_images();
    }

    /// 开始收集本次显示所需的协议 placement，尚不向终端发送。
    pub(crate) fn begin_preparation(&mut self) {
        self.graphics_placements.clear();
    }

    /// 发布本次显示的资源集合；即使不绘制也完成保活和需求调度。
    pub(crate) fn finish_preparation(&mut self) {
        for url in self.cache.advance_frame() {
            self.discard_derived(&url);
        }
        self.preview_images.advance_frame();
        self.terminal_images.advance_frame();
        self.refresh_preview_targets();
        self.schedule_decode_demand();
    }

    /// 返回正在执行的图片 preview 与 decode 总数。
    pub(crate) fn loading_count(&self) -> usize {
        self.pending.len()
    }

    /// 把上一帧渲染观察到的 preview 尺寸交给本拍预取，并清空观察集合。
    fn refresh_preview_targets(&mut self) {
        self.preview_targets = self
            .observed_preview_targets
            .drain()
            .collect::<Vec<PreviewTarget>>();
        self.preview_targets.sort_by_key(|target| target.sort_key());
    }

    /// 记录稳定布局实际使用的 preview 尺寸，供下一拍半径预取复用。
    ///
    /// # Params:
    ///   - `area`: 已按终端几何收成正方形的图片区域
    pub(crate) fn observe_preview_target(&mut self, area: Rect) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        self.observed_preview_targets.insert(PreviewTarget::cover(
            (area.width, area.height),
            self.cell_pixels(),
            self.cell_fit(),
        ));
    }

    /// 记录行内封面的低清采样上限；实际 URL 仍由配置半径内的 prefetch 候选决定。
    pub(super) fn observe_thumbnail_target(&mut self) {
        self.observed_preview_targets
            .insert(PreviewTarget::thumbnail());
    }

    /// 返回行内封面复用的低清 preview 键。
    pub(super) fn thumbnail_preview_key(&self, url: &MediaUrl) -> TerminalImageKey {
        PreviewTarget::thumbnail().key(ImageIdentity::Url(url.clone()))
    }

    /// 为 URL 与目标区域构造 preview 缓存键。
    ///
    /// # Params:
    ///   - `url`: 图片源 URL
    ///   - `area`: preview 的目标 cell 区域
    pub(crate) fn preview_key(&self, url: &MediaUrl, area: Rect) -> TerminalImageKey {
        PreviewTarget::cover(
            (area.width, area.height),
            self.cell_pixels(),
            self.cell_fit(),
        )
        .key(ImageIdentity::Url(url.clone()))
    }

    /// 返回图片与尺寸是否仍需要生成 preview。
    fn should_prepare_preview(&self, url: &MediaUrl, key: &TerminalImageKey) -> bool {
        !self.cache.contains_key(url)
            && !self.preview_images.contains(key)
            && !self.preview_failures.contains(url)
            && !self.pending.contains(url)
            && !self.decode_demand.contains(url)
    }

    /// 登记一张实际显示或显式 prepare 所需的图片；下次 tick 按需解码。
    ///
    /// # Params:
    ///   - `url`: 需要解码像素的图片 URL
    pub(crate) fn demand_decode(&mut self, url: &MediaUrl) {
        if self.cache.contains_key(url) || self.decode_failures.contains(url) {
            return;
        }
        self.decode_demand.insert(url.clone());
    }

    /// 为非渲染消费者登记来源并立即请求解码图。
    ///
    /// # Params:
    ///   - `candidates`: 需要解码像素的来源与 URL
    pub(crate) fn load(&mut self, candidates: impl IntoIterator<Item = (SourceKind, MediaUrl)>) {
        for (source, url) in candidates {
            self.source_by_url.insert(url.clone(), source);
            self.decode_demand.insert(url.clone());
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
                    cell_width = target.cells().0,
                    cell_height = target.cells().1,
                    pixel_width = target.pixels().width(),
                    pixel_height = target.pixels().height(),
                    cell_fit = ?key.cell_fit(),
                    "generate cover preview"
                );
                if !self.workers.fetcher.preview(source, url.clone(), *target) {
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
    ///   - `url`: 需要解码像素的图片 URL
    fn request_decode(&mut self, url: &MediaUrl) {
        if self.cache.contains_key(url) || self.decode_failures.contains(url) {
            self.decode_demand.remove(url);
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
        if !self.workers.fetcher.decode(
            source,
            url.clone(),
            self.cfg.cover().decode_pixels().clone(),
        ) {
            self.pending.remove(url);
            self.decode_demand.remove(url);
            self.decode_failures.insert(url.clone());
        }
    }

    /// 结束对应内容的请求；过期成功结果直接释放，失败按内容与目标保留且不重试。
    fn install_terminal_image(&mut self, completion: EncodeCompletion) {
        let EncodeCompletion { identity, result } = completion;
        self.encode_pending.remove(&identity);
        let current = self.encode_input(&identity.key) == Some(identity.input);
        match result {
            Ok(image) if current => {
                let bytes = image.resident_bytes();
                mineral_log::debug!(target: "cover_cache", ?identity, bytes, "terminal image ready");
                self.terminal_images.insert(&identity.key, image, bytes);
            }
            Ok(_) => {
                mineral_log::debug!(target: "cover_cache", ?identity, "discard terminal image with obsolete input");
            }
            Err(error) => {
                mineral_log::warn!(target: "cover", ?identity, current,
                    error = mineral_log::chain(&error), "terminal image encoding failed");
                self.encode_failures.insert(identity, error);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use image::{DynamicImage, RgbImage};
    use mineral_model::MediaUrl;

    use super::ImageEngine;
    use crate::image::CoverFingerprint;
    use crate::image::fetch::{CoverCompletion, CoverPreviewReady, CoverReady};
    use crate::image::halfblock::HalfblockImage;
    use crate::image::key::{ImageIdentity, PixelSize, PreviewTarget, TerminalImageKey};

    /// 造一个使用默认配置的测试图片引擎(不起 worker)。
    fn engine() -> color_eyre::Result<ImageEngine> {
        let cfg = Arc::new(crate::config::TuiConfig::defaults()?);
        Ok(ImageEngine::disabled(cfg))
    }

    /// 造一个 preview 完成(不带显示图,JPEG 语义)。
    fn preview_ready(url: &MediaUrl) -> CoverPreviewReady {
        let key = TerminalImageKey::rasterized(
            ImageIdentity::Url(url.clone()),
            PixelSize::from_cells(/*cells*/ (4, 4), /*cell_pixels*/ (8, 16)),
            (4, 4),
            crate::config::CoverCellFit::Crop,
        );
        CoverPreviewReady {
            url: url.clone(),
            key,
            image: HalfblockImage::test_image(),
            bytes: 1,
        }
    }

    /// 非 JPEG 源的一次解码双产物落地:preview 与显示图一起装好,显示图满足 decode demand。
    #[test]
    fn combined_preview_installs_preview_and_full_image() -> color_eyre::Result<()> {
        let mut engine = engine()?;
        let url = MediaUrl::remote("https://example.com/c.png")?;
        let image = Arc::new(DynamicImage::ImageRgb8(RgbImage::new(16, 16)));
        engine.decode_demand.insert(url.clone());
        engine.pending.insert(url.clone());

        engine.apply_completion(CoverCompletion::PreviewAndDecoded {
            preview: preview_ready(&url),
            full: CoverReady {
                url: url.clone(),
                fingerprint: CoverFingerprint::of(&image)?,
                image,
                palette: None,
            },
        });

        assert!(engine.cache.contains_key(&url), "显示图应进 RAM LRU");
        assert!(!engine.pending.contains(&url), "in-flight 应结束");
        assert!(
            !engine.decode_demand.contains(&url),
            "decode demand 应被显示图满足"
        );
        Ok(())
    }

    /// 行内封面只消费预取结果;方图与矩形图都保留原比例,未预取的行不下载或完整解码。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn thumbnails_use_prefetched_pixels_without_full_decode() -> color_eyre::Result<()> {
        use crate::image::{ImageRenderPhase, fetch::CoverFetcher, graphics::TerminalGraphics};
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;
        use std::time::Duration;

        let sample_limit = PreviewTarget::thumbnail().pixels();
        for (width, height, width_divisor, height_divisor) in
            [(512, 512, 1, 1), (256, 512, 2, 1), (512, 256, 1, 2)]
        {
            let dir = tempfile::tempdir()?;
            let path = dir.path().join("cover.jpg");
            image::RgbImage::from_fn(width, height, |x, y| {
                image::Rgb([
                    if x < 256 { 220 } else { 20 },
                    if y < 256 { 180 } else { 40 },
                    80,
                ])
            })
            .save(&path)?;
            let url = MediaUrl::local(path);
            let outside_radius = MediaUrl::remote("https://example.com/not-prefetched.jpg")?;
            let cfg = Arc::new(crate::config::TuiConfig::defaults()?);
            let fetcher = CoverFetcher::spawn(cfg.cover().clone(), 0, None).await?;
            let mut engine =
                ImageEngine::new(cfg, fetcher, TerminalGraphics::fixed_kitty(), (8, 16));
            assert_ne!(
                engine.thumbnail_preview_key(&url),
                engine.preview_key(&url, Rect::new(0, 0, 4, 2)),
                "32×32 采样图不能与相同物理尺寸的 4×2 cell preview 混用"
            );
            let area = Rect::new(0, 0, 1, 1);
            let mut buffer = Buffer::empty(area);
            engine.prepare_and_render_thumbnail(
                Some(&outside_radius),
                area,
                &mut buffer,
                ImageRenderPhase::Stable,
            );
            engine.tick();
            engine.prefetch([(mineral_model::SourceKind::NETEASE, url.clone())]);

            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    engine.tick();
                    engine.prepare_and_render_thumbnail(
                        Some(&url),
                        area,
                        &mut buffer,
                        ImageRenderPhase::Stable,
                    );
                    if buffer
                        .cell((0, 0))
                        .is_some_and(|cell| cell.symbol().starts_with('\u{10EEEE}'))
                    {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await?;

            assert!(engine.decode_demand.is_empty(), "行缩略图不能触发完整解码");
            assert!(engine.pending.is_empty(), "只有显式预取的封面应完成请求");
            assert!(!engine.source_by_url.contains_key(&outside_radius));
            assert!(
                !engine.cache.contains_key(&url),
                "JPEG 缩略图不能占用完整图缓存"
            );
            let commands = engine.take_graphics_commands();
            let key = engine.thumbnail_preview_key(&url);
            let image = engine
                .terminal_images
                .get(&key)
                .ok_or_else(|| color_eyre::eyre::eyre!("missing inline image"))?;
            assert!(
                image.resident_bytes()
                    <= u64::from(sample_limit.width()) * u64::from(sample_limit.height()) * 4,
                "行内 shared memory 不超过低清像素的 RGBA 预算"
            );
            assert!(commands.contains("a=t,"), "图片传输须在 cell 外排队");
            assert!(commands.contains("a=p,U=1,c=1,r=1"));
            assert!(
                commands.contains(&format!(
                    "s={},v={};",
                    sample_limit.width() / width_divisor,
                    sample_limit.height() / height_divisor
                )),
                "传输应保留无补边的小图比例,显示尺寸交给 Kitty: {commands:?}"
            );
            engine.prepare_and_render_thumbnail(
                Some(&url),
                area,
                &mut buffer,
                ImageRenderPhase::Stable,
            );
            assert!(
                engine.take_graphics_commands().is_empty(),
                "重画不重复传输或建立 placement"
            );
            let mut expanded = Buffer::empty(Rect::new(0, 0, 2, 1));
            engine.prepare_and_render_thumbnail(
                Some(&url),
                expanded.area,
                &mut expanded,
                ImageRenderPhase::Stable,
            );
            let resized = engine.take_graphics_commands();
            assert!(resized.contains("a=p,U=1,c=2,r=1"));
            assert!(
                !resized.contains("a=t,"),
                "拓宽图片列只更新 placement,不重传像素"
            );
            assert_ne!(
                expanded.cell((0, 0)).map(ratatui::buffer::Cell::symbol),
                expanded.cell((1, 0)).map(ratatui::buffer::Cell::symbol),
                "两格必须寻址图片的不同列"
            );

            let full = Arc::new(DynamicImage::ImageRgb8(RgbImage::new(64, 64)));
            engine.install_decoded_cover(CoverReady {
                url: url.clone(),
                fingerprint: CoverFingerprint::of(&full)?,
                image: full,
                palette: None,
            });
            engine.set_budgets(0, 16 * 1024, 16 * 1024);
            assert!(!engine.cache.contains_key(&url));
            assert!(
                engine.terminal_images.contains(&key),
                "完整图回填和逐出不得清掉独立缩略图"
            );
        }
        Ok(())
    }

    /// 离屏、缩放与滚动不启动行内编码；非 Kitty 不注册采样尺寸。
    #[test]
    fn thumbnail_render_phases_do_not_create_hidden_work() -> color_eyre::Result<()> {
        use crate::image::ImageRenderPhase;
        use ratatui::buffer::Buffer;
        use ratatui::layout::Rect;

        let cfg = Arc::new(crate::config::TuiConfig::defaults()?);
        let mut kitty = ImageEngine::disabled_kitty(cfg);
        let url = MediaUrl::remote("https://example.com/cover.jpg")?;
        kitty.cache.insert_test(
            &url,
            Arc::new(DynamicImage::ImageRgb8(RgbImage::new(64, 64))),
        )?;
        let area = Rect::new(0, 0, 1, 1);
        for phase in [
            ImageRenderPhase::Offscreen,
            ImageRenderPhase::Resizing,
            ImageRenderPhase::Scrolling,
        ] {
            kitty.prepare_and_render_thumbnail(Some(&url), area, &mut Buffer::empty(area), phase);
            assert!(kitty.encode_pending.is_empty());
            assert!(kitty.decode_demand.is_empty());
            assert!(kitty.take_graphics_commands().is_empty());
        }
        kitty.insert_test_thumbnail(&url)?;
        let mut first = Buffer::empty(area);
        kitty.prepare_and_render_thumbnail(
            Some(&url),
            area,
            &mut first,
            ImageRenderPhase::Offscreen,
        );
        let commands = kitty.take_graphics_commands();
        assert!(commands.contains("a=t,"), "首次离屏绘制也须排入图片传输");
        assert!(commands.contains("a=p,U=1,c=1,r=1"));
        for phase in [
            ImageRenderPhase::Offscreen,
            ImageRenderPhase::Resizing,
            ImageRenderPhase::Scrolling,
            ImageRenderPhase::Stable,
        ] {
            let moved_area = Rect::new(3, 2, area.width, area.height);
            let mut moved = Buffer::empty(moved_area);
            kitty.prepare_and_render_thumbnail(Some(&url), moved_area, &mut moved, phase);
            assert_eq!(moved.cell((3, 2)), first.cell((0, 0)), "移动只改变屏幕位置");
            assert!(
                kitty.take_graphics_commands().is_empty(),
                "移动不得重复传输图片或重建 placement"
            );
            assert!(kitty.encode_pending.is_empty());
            assert!(kitty.decode_demand.is_empty());
        }
        let mut other = engine()?;
        other.prepare_and_render_thumbnail(
            Some(&url),
            area,
            &mut Buffer::empty(area),
            ImageRenderPhase::Stable,
        );
        assert!(other.observed_preview_targets.is_empty());
        Ok(())
    }

    /// 纯 preview(JPEG 语义)只进 preview LRU,不占显示图缓存。
    #[test]
    fn plain_preview_does_not_populate_full_image_cache() -> color_eyre::Result<()> {
        let mut engine = engine()?;
        let url = MediaUrl::remote("https://example.com/c.jpg")?;

        engine.apply_completion(CoverCompletion::Preview(preview_ready(&url)));

        assert!(
            !engine.cache.contains_key(&url),
            "JPEG preview 不应占显示图缓存"
        );
        Ok(())
    }
}

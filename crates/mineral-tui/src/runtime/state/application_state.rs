//! 应用全局状态的构造、配置换算与逐帧动画推进。
//!
//! 歌单曲目缓存缺少某个 key 表示尚未拉取到数据，渲染继续显示 loading。

use std::sync::Arc;

use mineral_model::SourceKind;
use mineral_spectrum::SpectrumComputer;
use ratatui::layout::Rect;
use rustc_hash::FxHashMap;

use crate::components::layout::browse::spectrum::SpectrumState;
use crate::components::layout::shared::transport::TransportFeedback;
use crate::render::anim::{Toggle, ticks16_from_ms};
use crate::runtime::marquee::Marquees;
use crate::runtime::playback::Playback;

use super::{
    BrowsePage, ImageEngine, LibraryData, OverlayReveal, PlayerMirror, SearchPage, search_whitelist,
};

/// 应用顶层状态。
pub struct AppState {
    /// Browse 布局层的 view 状态:视图切换 + 全屏子模式 + 列表导航 + 歌词 + `/` 过滤,
    /// 由 [`BrowsePage`] 聚合。与 model 数据(library / player)分离。
    pub browse: BrowsePage,

    /// channel 搜索布局态:与 [`BrowsePage::fullscreen`] 同级的全屏级布局态(两者逻辑 `on` 互斥)。
    /// 含布局开关 + 当前源 + 输入焦点 + 焦点环 + per-源会话。
    pub channel_search: SearchPage,

    /// 顶栏失焦变灰:`on()` = 已变灰(终端未聚焦)、`eased_in_out()` = 变灰深度。
    /// 终端聚焦态由 [`Self::focused`] 反读;初始 `off`(聚焦)——mode 1004 只报变化、
    /// 不支持的终端永不发事件,降级方向必须是「恒聚焦」。
    pub dim: Toggle,

    /// server 数据镜像/拉取缓存(歌单 / 曲目 / 歌词 + ♥/本地完整播放次数装饰)。
    pub library: LibraryData,

    /// 脚本下发的窗口标题整串覆盖(`Event::WindowTitleOverride` 落地;
    /// `None` = 无覆盖,标题走结构化模板)。渲染产物直通,不属于配置。
    pub window_title_override: Option<String>,

    /// server 权威播放态镜像(在播歌 / 队列 / 洗牌备份 / 同步版本号)。
    pub player: PlayerMirror,

    /// 播放状态机。
    pub playback: Playback,

    /// 播放栏本地反馈；动作唤起，显式 tick 推进，绘制只读。
    pub(crate) transport: TransportFeedback,

    /// 频谱状态(条高 + 平滑)。
    pub spectrum: SpectrumState,

    /// 频谱 FFT 计算器:吃 PCM 样本,出 64 根条的目标高度。
    pub fft: SpectrumComputer,

    /// 图片引擎状态(原图/色板缓存、在飞集合与终端成品)。
    pub images: ImageEngine,

    /// 后台 server scheduler 当前快照(每 tick 由 App 从 `Client::task_snapshot`
    /// 灌入)。**只含**:server 端 ChannelFetch lane(playlists / tracks /
    /// song-url / lyrics / liked)。封面由 client-local 的 [`ImageEngine`] 管理,
    /// 不在这里。
    /// `by_kind` 给 top_status 显示「pl:N tr:N ...」按 kind 拆分用。
    pub tasks_snapshot: mineral_task::Snapshot,

    /// Small download summary polled every tick.
    pub downloads_summary: mineral_protocol::DownloadSummary,

    /// Flat Song download snapshot, refreshed only while Downloads overlay is open.
    pub downloads: Vec<mineral_protocol::SongDownloadView>,

    /// 已加载的全局配置(`Arc` 共享只读):渲染 / 运行时模块经此读各段旋钮
    /// (lyrics 行距、layout 阈值、prefetch 半径、animation 时长等)。
    pub cfg: Arc<mineral_config::Config>,

    /// 上一帧的主帧面积(渲染端每帧回写,`Cell` 因渲染只持 `&AppState`)。
    /// 按键路径据此重算布局求锚点(如弹菜单贴选中行);首帧前为零矩形,
    /// 消费方需容忍空值(placement 的 clamp 兜底)。
    pub frame_area: std::cell::Cell<Rect>,

    /// 本帧的本地钟点(主循环每 tick 写 `Local::now()`;测试注入固定值)。
    /// 队列剩余时长的「预计播完钟点」据此算——渲染只持 `&AppState`,故走 `Cell`,
    /// 也让依赖 wall-clock 的快照可固定时间不飘。
    pub now: std::cell::Cell<chrono::DateTime<chrono::Local>>,

    /// 正在渲染的这一层浮层的入场进度,以及压在它上面那层的进度(千分比,渲染端每帧
    /// 回写,`Cell` 因渲染只持 `&AppState`)。高亮交接据此插值:被压住的一层随上层
    /// 入场把选中高亮淡出,上层则同步淡入,两处共用一个进度故不会错拍。
    pub overlay_reveal: std::cell::Cell<OverlayReveal>,

    /// 各源能力声明镜像(启动时从 server 拉一次)。UI 据此决定渲染哪些入口
    /// (搜索类型 / 歌单写操作键 / 网页链接复制项);缺项 = 该源未注册,入口不画。
    pub caps: FxHashMap<SourceKind, mineral_channel_core::ChannelCaps>,

    /// 溢出标题滚动(marquee)的槽相位状态(节奏来自配置 `animation.marquee_*`)。
    pub(crate) marquees: Marquees,

    /// not playing 待机唱片纹的旋转状态(相位每 tick 推进,节奏来自配置
    /// `animation.vinyl_rev_ms`)。
    pub(crate) vinyl: crate::components::layout::shared::vinyl::VinylSpin,
}

impl AppState {
    /// 构造空状态(所有列表 / 缓存初始为空,等 [`AppState::apply`] 增量填充);
    /// 过渡时长 / 频谱旋钮 / 各段手感由注入的配置落地。
    ///
    /// # Params:
    ///   - `cfg`: 已加载的全局配置(`Arc` 共享,渲染/运行时模块经 `state.cfg` 读)
    ///   - `images`: 已完成 worker 与 terminal backend 接线的图片引擎
    pub fn new(cfg: Arc<mineral_config::Config>, images: ImageEngine) -> Self {
        let anim = cfg.tui().animation();
        let tick_ms = *anim.frame_tick_ms();
        let playback = Playback::new();
        let transport = TransportFeedback::new(playback.mode, anim);
        Self {
            browse: BrowsePage::new(anim),
            channel_search: SearchPage::new(
                ticks16_from_ms(*anim.fullscreen_ms(), tick_ms),
                ticks16_from_ms(*anim.search_focus_morph_ms(), tick_ms),
            )
            .with_whitelist(search_whitelist::SearchWhitelist::from(
                cfg.tui().search().channel(),
            )),
            dim: Toggle::new(ticks16_from_ms(*anim.focus_fade_ms(), tick_ms)),
            library: LibraryData::new(),
            window_title_override: None,
            player: PlayerMirror::new(),
            playback,
            transport,
            spectrum: SpectrumState::new(cfg.tui().spectrum().clone(), tick_ms),
            fft: SpectrumComputer::new(spectrum_params(cfg.tui().spectrum())),
            images,
            tasks_snapshot: mineral_task::Snapshot {
                running: 0,
                by_kind: FxHashMap::default(),
            },
            downloads_summary: mineral_protocol::DownloadSummary::default(),
            downloads: Vec::new(),
            marquees: Marquees::from_config(anim.marquee(), tick_ms),
            vinyl: crate::components::layout::shared::vinyl::VinylSpin::from_config(
                *anim.vinyl_rev_ms(),
                tick_ms,
            ),
            cfg,
            frame_area: std::cell::Cell::new(Rect::default()),
            now: std::cell::Cell::new(chrono::Local::now()),
            overlay_reveal: std::cell::Cell::new(OverlayReveal::default()),
            caps: FxHashMap::default(),
        }
    }

    /// 测试构造:defaults 配置(= 接线前硬编码常量)的空状态。
    #[cfg(test)]
    pub(crate) fn test_default() -> color_eyre::Result<Self> {
        let cfg = Arc::new(mineral_config::Config::defaults()?);
        Ok(Self::test_with_config(cfg))
    }

    /// 测试构造：用指定配置与禁用 worker 的图片引擎创建空状态。
    #[cfg(test)]
    pub(crate) fn test_with_config(cfg: Arc<mineral_config::Config>) -> Self {
        let images = ImageEngine::disabled(Arc::clone(&cfg));
        Self::new(cfg, images)
    }

    /// 推进一帧的各动画 / 相位状态(主循环每 tick 恰调一次):视图切换扫入、全屏形变、
    /// 氛围背景滞后跟随、搜索布局、marquee 相位、失焦渐变、歌词滚动。
    pub fn tick_frame(&mut self) {
        self.browse.view.tick();
        self.browse.list_expansion.get_mut().tick();
        self.browse.fullscreen.tick();
        self.browse.lyric_view.extra_press.tick();
        self.browse.tick_ambient_reveal();
        self.channel_search.tick();
        self.marquees.tick();
        self.vinyl.tick();
        self.dim.tick();
        self.playback.tick_envelope_reveal();
        self.transport.tick(
            self.playback.mode,
            self.cfg.tui().animation(),
            std::time::Instant::now(),
        );
        self.tick_lyric_scroll();
    }

    /// 波形入场揭示动画的全程拍数(`tui.waveform.reveal.duration_ms` 现读折算)。
    ///
    /// # Return:
    ///   拍数,`1..=u16::MAX`(0ms 也占一拍,语义 = 一帧到位)。
    pub(crate) fn waveform_reveal_ticks(&self) -> u16 {
        crate::render::anim::ticks16_from_ms(
            *self.cfg.tui().waveform().reveal().duration_ms(),
            *self.cfg.tui().animation().frame_tick_ms(),
        )
    }

    /// 光标与列表视口上下边缘的最小行距(配置 `behavior.scrolloff`)。
    pub(crate) fn scrolloff(&self) -> usize {
        usize::from(*self.cfg.tui().behavior().scrolloff())
    }

    /// 现读 minimap 光标移动时长与帧间隔；在途动画每帧使用该值重设速度。
    pub(crate) fn minimap_cursor_ticks(&self) -> u16 {
        let anim = self.cfg.tui().animation();
        ticks16_from_ms(*anim.minimap_cursor_ms(), *anim.frame_tick_ms())
    }

    /// 列表视口滚动平移的缓动拍数(配置 `animation.list_scroll_ms` 折算)。
    pub(crate) fn list_glide_ticks(&self) -> u16 {
        let anim = self.cfg.tui().animation();
        ticks16_from_ms(*anim.list_scroll_ms(), *anim.frame_tick_ms())
    }
}

/// 把配置的频谱段映射成 DSP 参数([`mineral_spectrum::SpectrumParams`])。
/// mineral-spectrum 是叶子 crate 不依赖配置,在此(消费侧)做一次显式映射。
///
/// # Params:
///   - `cfg`: 频谱段配置
///
/// # Return:
///   DSP 参数。
pub(crate) fn spectrum_params(
    cfg: &mineral_config::SpectrumConfig,
) -> mineral_spectrum::SpectrumParams {
    mineral_spectrum::SpectrumParams::builder()
        .fft_size(*cfg.fft_size())
        .f_min(*cfg.f_min())
        .f_max(*cfg.f_max())
        .log_axis_blend(*cfg.log_axis_blend())
        .db_floor(*cfg.db_floor())
        .db_ceil(*cfg.db_ceil())
        .peak_mix(*cfg.peak_mix())
        .build()
}

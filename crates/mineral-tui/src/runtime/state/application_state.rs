//! 应用根状态的所有权组合；组件不会接收该聚合类型。

use super::{models::AppModels, resources::ClientResources};
use crate::image::ImageEngine;
use crate::render::anim::ticks16_from_ms;
use crate::view::RootView;
use std::sync::Arc;

/// 组合模型、组件树和资源服务；跨领域事件在应用层路由。
pub struct AppState {
    /// 后端事实及领域缓存。
    pub(crate) models: AppModels,

    /// 自己保留交互状态的组件实例。
    pub(crate) ui: RootView,

    /// 图片任务、终端协议及计算资源。
    pub(crate) resources: ClientResources,

    /// 当前有效配置，整树替换。
    pub(crate) cfg: Arc<mineral_config::Config>,
}

impl AppState {
    /// 装配独立的模型、界面和资源所有者。
    pub fn new(cfg: Arc<mineral_config::Config>, images: ImageEngine) -> Self {
        let models = AppModels::new();
        let ui = RootView::new(&cfg, models.playback.mode);
        let resources = ClientResources::new(images, &cfg);
        Self {
            models,
            ui,
            resources,
            cfg,
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
        self.ui.browse.view.tick();
        self.ui.browse.playlists.tick();
        self.ui.browse.tracks.tick();
        self.ui.browse.fullscreen.tick();
        self.ui.browse.lyrics.extra_press.tick();
        self.ui.browse.tick_ambient_reveal();
        self.ui.channel_search.tick();
        self.ui.vinyl.tick();
        self.ui.dim.tick();
        self.models.playback.tick_envelope_reveal();
        self.ui.transport.tick(
            self.models.playback.mode,
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

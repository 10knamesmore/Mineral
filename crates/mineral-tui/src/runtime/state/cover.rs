//! 当前播放封面的配色身份与全屏切歌转场；图片资源由图片引擎持有。

use mineral_model::MediaUrl;
use mineral_protocol::AdvanceKind;

use crate::config::CoverTransitionConfig;
use crate::image::ReadyImages;
use crate::render::anim::{Transition, ticks16_from_ms};
use crate::render::palette::CoverPalette;

/// 已应用到频谱、动态主题与氛围背景的封面配色。
/// 色板缺失表示该封面已完成取色但没有可用颜色，不能当成仍在加载。
pub(crate) struct AppliedCoverPalette {
    pub(crate) url: MediaUrl,

    /// 独立保留正在使用的色板，解码缓存逐出不改变屏上的配色目标。
    pub(crate) palette: Option<CoverPalette>,
}

/// 全屏切歌时正在交叉合成的两张封面。
#[derive(Clone, PartialEq)]
pub(crate) struct CoverTransition {
    pub(crate) from_url: MediaUrl,

    pub(crate) to_url: MediaUrl,

    /// 上一首反向推进，其余进入方式沿下一首方向。
    pub(crate) advance: Option<AdvanceKind>,

    pub(crate) anim: Transition,
}

/// 根界面的封面显示状态，与下载、编码和资源逐出生命周期独立。
#[derive(Default)]
pub(crate) struct CoverPresentation {
    pub(crate) applied_palette: Option<AppliedCoverPalette>,

    pub(crate) transition: Option<CoverTransition>,

    /// 上一拍显示的封面身份，用于确定下一次转场的退场图片。
    displayed_cover: Option<MediaUrl>,
}

impl CoverPresentation {
    /// 推进切歌转场；只有稳态全屏且新旧图片都已解码时开始合成。
    pub(crate) fn tick(
        &mut self,
        current_cover: Option<MediaUrl>,
        advance: Option<AdvanceKind>,
        fullscreen_stable: bool,
        images: ReadyImages<'_>,
        config: &CoverTransitionConfig,
        frame_tick_ms: u64,
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
        let previous = std::mem::replace(&mut self.displayed_cover, current_cover);
        self.transition = None;
        let (Some(from_url), Some(to_url)) = (previous, self.displayed_cover.as_ref()) else {
            return;
        };
        if !*config.enabled()
            || !images.contains_decoded(&from_url)
            || !images.contains_decoded(to_url)
        {
            return;
        }
        self.transition = Some(CoverTransition {
            from_url,
            to_url: to_url.clone(),
            advance,
            anim: Transition::expanding(ticks16_from_ms(*config.duration_ms(), frame_tick_ms)),
        });
    }
}

//! 客户端执行器与计算资源；组件只取得就绪资源或声明需求的能力。

use crate::image::ImageEngine;
use crate::runtime::cover_collage::CoverCollages;
use mineral_spectrum::SpectrumComputer;

/// 独立于模型与界面生命周期的客户端资源。
pub(crate) struct ClientResources {
    /// 图片引擎状态(原图/色板缓存、在飞集合与终端成品)。
    pub(crate) images: ImageEngine,

    /// 聚合歌单的封面成员选择与渐进拼贴记录。
    pub(crate) collages: CoverCollages,

    /// 频谱 FFT 计算器:吃 PCM 样本,出 64 根条的目标高度。
    pub(crate) fft: SpectrumComputer,
}

impl ClientResources {
    /// 连接既有图片管线，并初始化当前配置所需的 FFT。
    pub(crate) fn new(images: ImageEngine, config: &crate::config::TuiConfig) -> Self {
        Self {
            images,
            collages: CoverCollages::default(),
            fft: SpectrumComputer::new(super::spectrum_params(config.spectrum())),
        }
    }
}

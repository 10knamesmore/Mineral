//! 封面段(挂在 `TuiConfig` 下):显示适配、抓取、缓存、并发与 kmeans 取色参数。

use std::num::NonZeroU32;

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

use mineral_config::de;

/// 封面配置。
#[config_section]
pub struct CoverConfig {
    /// 封面图像协议；Kitty 需要 POSIX 共享内存
    protocol: CoverProtocolMode,

    /// 主封面边缘与字符格的适配；不改变横图、竖图的整体外框，不影响行内缩略图
    cell_fit: CoverCellFit,

    /// 封面下载超时
    http_timeout_secs: u64,

    /// 高清封面加载防抖
    debounce_ms: u64,

    /// 下载并发数，至少 1
    download_workers: usize,

    /// 编码并发数，至少 1
    encode_workers: usize,

    /// 解码目标尺寸，修改不影响已缓存图片
    decode_pixels: CoverDecodePixelsConfig,

    /// 封面取色
    kmeans: KmeansConfig,

    /// 封面缓存容量
    cache: CoverCacheConfig,
}

/// 解码目标尺寸；保持比例，小图不放大
#[config_section]
#[derive(typed_builder::TypedBuilder)]
pub struct CoverDecodePixelsConfig {
    /// 目标宽度（像素），须大于 0
    #[lua_type("integer")]
    width: NonZeroU32,

    /// 目标高度（像素），须大于 0
    #[lua_type("integer")]
    height: NonZeroU32,
}

/// 封面缓存预算；可见图片可暂超内存预算
#[config_section]
pub struct CoverCacheConfig {
    /// 原图磁盘容量（字节）
    #[serde(deserialize_with = "de::u64_lossy")]
    disk: u64,

    /// 高清像素内存容量（字节）
    #[serde(deserialize_with = "de::u64_lossy")]
    image: u64,

    /// 预览图内存容量（字节）
    #[serde(deserialize_with = "de::u64_lossy")]
    preview: u64,

    /// 终端图像内存容量（字节）
    #[serde(deserialize_with = "de::u64_lossy")]
    protocol: u64,
}

/// 封面图像协议
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum CoverProtocolMode {
    /// 自动探测，不支持时使用半块字符
    Auto,

    /// 半块字符
    Halfblocks,

    /// Kitty；无 POSIX 共享内存时沿用自动探测结果
    Kitty,

    /// Sixel
    Sixel,

    /// iTerm2
    Iterm2,
}

/// 主封面按原图比例确定最小字符外框后，如何处理不足一格的边缘。
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum CoverCellFit {
    /// 裁剪
    Crop,

    /// 拉伸
    Stretch,

    /// 保持原图, 留白
    Contain,
}

/// 全屏封面转场
#[config_section]
pub struct CoverTransitionConfig {
    /// 是否启用
    enabled: bool,

    /// 转场样式
    style: CoverTransitionStyle,

    /// 转场时长
    duration_ms: u32,

    /// 缩放转场参数
    zoom: ZoomConfig,
}

/// 缩放转场
#[config_section]
pub struct ZoomConfig {
    /// 缩放倍数；1 为仅淡入淡出
    scale: f32,
}

/// 全屏封面转场样式
#[lua_enum]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum CoverTransitionStyle {
    /// 淡入淡出
    Fade,

    /// 向左推入
    Slide,

    /// 缩放切换
    Zoom,
}

/// 封面取色
#[config_section]
pub struct KmeansConfig {
    /// 采样边长（像素）
    sample_dim: u32,

    /// 色板颜色数，至少 1
    swatches: usize,

    /// 取色随机种子，固定可复现
    seed: u64,

    /// 最大迭代次数
    max_iter: usize,

    /// 质心位移收敛阈值
    converge: f32,

    /// 明度下限（Lab L，0-100）
    l_min: f32,

    /// 明度上限（Lab L，0-100）
    l_max: f32,

    /// 彩度下限（Lab √(a²+b²)）
    chroma_min: f32,

    /// 有效像素最小百分比，不足则取消过滤
    min_valid_pixels_pct: usize,
}

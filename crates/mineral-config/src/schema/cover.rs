//! 封面段(挂在 `TuiConfig` 下):抓取 / 缓存 / 并发 + kmeans 取色参数。
//!
//! [`CoverStorageMode`] 与渲染层存储模式语义对齐,但保持解耦——接线处做映射。

use mineral_config_macros::config_section;
use serde::Deserialize;

/// 封面配置。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct CoverConfig {
    /// 封面下载 HTTP 超时(秒)。
    http_timeout_secs: u64,

    /// 封面缓存最大边长(像素)。
    max_dim: u32,

    /// 封面 JPEG 重编码质量(0-100)。
    jpeg_quality: u8,

    /// 封面磁盘存储模式。
    storage: CoverStorageMode,

    /// 封面切换去抖(毫秒)。
    debounce_ms: u64,

    /// 封面下载并发 worker 数。
    download_workers: usize,

    /// 封面编码并发 worker 数。
    encode_workers: usize,

    /// kmeans 取色参数。
    kmeans: KmeansConfig,
}

/// 封面磁盘存储模式。不依赖渲染 crate;接线处映射到具体实现。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum CoverStorageMode {
    /// 原始下载字节(无损原图,扩展名按字节嗅探)。
    Raw,

    /// 解码缩放后重编码 JPEG(体积小,锁定 ≤ `max_dim`)。
    Resized,
}

/// kmeans 取色参数(挂在 `CoverConfig` 下)。
///
/// 字段私有 + `#[non_exhaustive]`,经 getter 读取。
#[config_section]
pub struct KmeansConfig {
    /// 取色前先缩到的采样边长(像素);聚类只看颜色分布,无需全分辨率。
    sample_dim: u32,

    /// 提取的色板色数(聚类数 k)。
    swatches: usize,

    /// kmeans 随机种子(确定性复现)。
    seed: u64,

    /// kmeans 最大迭代次数。
    max_iter: usize,

    /// 收敛阈值(质心位移 < 此值即停)。
    converge: f32,

    /// 明度下限(Lab L,过滤过暗像素)。
    l_min: f32,

    /// 明度上限(Lab L,过滤过亮像素)。
    l_max: f32,

    /// 彩度下限(过滤灰像素)。
    chroma_min: f32,

    /// 有效像素占比下限(%);低于此放弃取色。
    min_valid_pixels_pct: usize,
}

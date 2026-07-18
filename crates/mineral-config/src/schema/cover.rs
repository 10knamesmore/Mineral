//! 封面段(挂在 `TuiConfig` 下):抓取 / 缓存 / 并发 + kmeans 取色参数。
//!
//! [`CoverStorageMode`] 与渲染层存储模式语义对齐,但保持解耦——接线处做映射。

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

use crate::schema::de;

/// 封面配置。
#[config_section]
pub struct CoverConfig {
    /// 单张封面下载 HTTP 超时(秒)。
    http_timeout_secs: u64,

    /// 封面解码后等比缩放到的最大边长(像素);终端显示 ~240px 足够,大了费内存。
    max_dim: u32,

    /// 封面 JPEG 重编码质量(1-100);**仅 `storage = "resized"` 时生效**。
    jpeg_quality: u8,

    /// 封面磁盘存储模式。
    storage: CoverStorageMode,

    /// 封面切换去抖(毫秒):列表滚动停稳此时长才开始渲染真图,期间显示程序化色块占位。
    debounce_ms: u64,

    /// 封面下载并发 worker 数,≥1。
    download_workers: usize,

    /// 封面终端协议编码并发 worker 数,≥1。
    encode_workers: usize,

    /// kmeans 取色参数(封面派生配色)。
    kmeans: KmeansConfig,

    /// 缓存预算(磁盘配额 + 两层 RAM 预算)。
    cache: CoverCacheConfig,
}

/// 封面缓存预算(挂在 `CoverConfig` 下)。三档都是 client 进程的旋钮:
/// 磁盘是跨进程共享的持久文件,两层 RAM 是本进程常驻内存。
#[config_section]
pub struct CoverCacheConfig {
    /// 磁盘缓存容量上限(字节),存原始/重编码封面文件;可写算式如 `4 * 1024 ^ 3`。
    #[serde(deserialize_with = "de::u64_lossy")]
    disk: u64,

    /// 解码原图 RAM 预算(字节)。区别于 `disk`(磁盘原始字节):这是常驻 RAM 的
    /// 解码位图,越界即逐出最久未显示的封面,把进程内存钉死在此数。
    #[serde(deserialize_with = "de::u64_lossy")]
    image: u64,

    /// 已编码终端协议 RAM 预算(字节)。是 `image` 之外的第二层常驻 RAM:
    /// 每个协议留着源图副本 + kitty/sixel 编码序列,全屏大图可达 MB 级。
    /// 越界即逐出最久未渲染的协议(可后台重编,不损正确性)。
    #[serde(deserialize_with = "de::u64_lossy")]
    protocol: u64,

    /// 同一张封面并存的已编码尺寸数,≥1;常规面板与全屏各占一份,
    /// 超出时逐出该封面最久未渲染的尺寸。
    sizes_per_image: usize,
}

/// 封面磁盘存储模式。不依赖渲染 crate;接线处映射到具体实现。
#[lua_enum]
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
#[config_section]
pub struct KmeansConfig {
    /// 取色前先缩到的采样边长(像素);聚类只看颜色分布,64 足够,调大只费 CPU。
    sample_dim: u32,

    /// 提取的色板色数(聚类数 k),≥1;色多层次细、色少更整体。
    swatches: usize,

    /// kmeans 随机种子(确定性复现);**必须固定**,否则同一封面每次取色不同、颜色会跳。
    seed: u64,

    /// kmeans 最大迭代次数;封面色块少,库推荐量级即可收敛。
    max_iter: usize,

    /// 收敛阈值(质心位移 < 此值即停);Lab 空间推荐 5.0。
    converge: f32,

    /// 明度下限(Lab L,0-100;过滤近黑像素,避免黑背景霸占色板)。
    l_min: f32,

    /// 明度上限(Lab L,0-100;过滤近白像素)。
    l_max: f32,

    /// 彩度下限 √(a²+b²)(过滤近灰像素;灰底对配色无贡献)。
    chroma_min: f32,

    /// 过滤后有效像素占比低于此(%)则放弃过滤改用全部像素,0-100;保证黑白封面也有色。
    min_valid_pixels_pct: usize,
}

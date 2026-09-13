//! 封面 worker 的结构化请求与完成事件。

use std::sync::Arc;

use image::DynamicImage;
use mineral_config::CoverDecodePixelsConfig;
use mineral_model::{MediaUrl, SourceKind};
use parking_lot::Mutex;

use crate::image::CoverFingerprint;
use crate::image::key::TerminalImageKey;
use crate::image::terminal::TerminalImage;
use crate::render::palette::CoverPalette;

/// 一次 preview worker 请求。
pub(super) struct PreviewRequest {
    /// 图片来源，决定 Remote 文件的缓存子目录。
    pub(super) source: SourceKind,

    /// 图片源 URL。
    pub(super) url: MediaUrl,

    /// 图片身份与目标像素尺寸组成的 preview 键。
    pub(super) key: TerminalImageKey,

    /// preview 对应的目标 cell 宽高。
    pub(super) cells: (u16, u16),
}

/// 一次按配置尺寸解码 worker 请求。
pub(super) struct DecodeRequest {
    /// 图片来源，决定 Remote 文件的缓存子目录。
    pub(super) source: SourceKind,

    /// 图片源 URL。
    pub(super) url: MediaUrl,

    /// 提交请求时的解码目标，取自当前有效配置。
    pub(super) pixels: CoverDecodePixelsConfig,
}

/// 图片 worker 的结构化请求。
pub(super) enum CoverRequest {
    /// 生成低清 preview。
    Preview(PreviewRequest),

    /// 按配置尺寸生成图片与色板。
    Decode(DecodeRequest),
}

/// worker 生成的一张真实封面低清 preview。
pub(crate) struct CoverPreviewReady {
    /// 封面来源 URL。
    pub url: MediaUrl,

    /// 图片身份与目标像素尺寸组成的 preview 键。
    pub key: TerminalImageKey,

    /// 可直接写入 ratatui buffer 的 halfblock preview。
    pub image: TerminalImage,

    /// preview RGB 像素缓冲常驻字节数。
    pub bytes: u64,
}

/// worker 完成一张封面的产物:图必有,色板尽力而为。
///
/// `palette` 为 `Option`，因此取色失败不会阻止封面图本身回传。
pub(crate) struct CoverReady {
    /// 封面来源 URL(= 缓存键 / drain 回填键)。
    pub url: MediaUrl,

    /// 解码后的内存图。
    pub image: Arc<DynamicImage>,

    /// 图片内容指纹；同一张图的不同 URL 靠它相认。
    pub fingerprint: CoverFingerprint,

    /// 从图提取的频谱色板;取色失败为 `None`(频谱回退 hue 漂移)。
    pub palette: Option<CoverPalette>,
}

/// 图片 worker 请求类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CoverRequestKind {
    /// 准备压缩源并生成真实低清 preview。
    Preview,

    /// 读取本地源数据并按配置尺寸生成图片与色板。
    Decode,
}

/// 一次图片 worker 完成事件。
pub(crate) enum CoverCompletion {
    /// 低清 preview 已经可以显示。
    Preview(CoverPreviewReady),

    /// 非 JPEG 源一次解码同时产出 preview 与显示图:preview 立即显示，显示图进入 RAM LRU。
    PreviewAndDecoded {
        /// 低清 preview 成品。
        preview: CoverPreviewReady,

        /// 复用同次解码的显示图与色板。
        full: CoverReady,
    },

    /// 图片已解码，可以进入 RAM LRU。
    Decoded(CoverReady),

    /// 请求失败；错误已在 worker 边界记录。
    Failed {
        /// 失败的图片 URL。
        url: MediaUrl,

        /// 失败请求的类型。
        kind: CoverRequestKind,
    },
}

/// 完成 buffer 类型别名。worker 端 push、client tick 端 drain。
pub(super) type ReadyBuf = Arc<Mutex<Vec<CoverCompletion>>>;

//! 组件使用的只读图片与需求收集；任务和协议状态由图片管线独占。

use mineral_model::MediaUrl;
use ratatui::{buffer::Buffer, layout::Rect};

use super::{ImageContent, ImageEngine, ImageRenderPhase, InlineImage};

/// 只读借用本次已就绪资源，不提供调度、预算或输出提交能力。
#[derive(Clone, Copy)]
pub(crate) struct ReadyImages<'a> {
    /// 图片管线的只读资源入口。
    engine: &'a ImageEngine,
}

impl ReadyImages<'_> {
    /// 绘制已经完成的图片或预览。
    pub(crate) fn render(
        self,
        content: ImageContent<'_>,
        area: Rect,
        target: &mut Buffer,
        phase: ImageRenderPhase,
    ) {
        self.engine.render(content, area, target, phase);
    }

    /// 已就绪的终端图片实际占据区域，用于背景避让。
    pub(crate) fn ready_area(self, url: &MediaUrl, area: Rect) -> Option<Rect> {
        self.engine.ready_area(url, area)
    }

    /// 两个身份是否对应同一份已解码像素。
    pub(crate) fn same_picture(self, from: &MediaUrl, to: &MediaUrl) -> bool {
        self.engine.same_picture(from, to)
    }

    /// 按当前终端比例求封面的方形区域。
    pub(crate) fn square_area(self, area: Rect) -> Rect {
        self.engine.square_area(area)
    }

    /// 是否已有完整解码像素，用于选择已完成的合成封面。
    pub(crate) fn contains_decoded(self, url: &MediaUrl) -> bool {
        self.engine.cache.contains_key(url)
    }

    /// 当前终端是否支持行内封面。
    pub(crate) fn supports_thumbnails(self) -> bool {
        self.engine.supports_thumbnails()
    }

    /// 把已有成品画到字符格；缺图时保留背景。
    pub(crate) fn render_thumbnail(self, url: Option<&MediaUrl>, area: Rect, target: &mut Buffer) {
        self.engine.render_thumbnail(url, area, target);
    }

    /// 保留已就绪图片的身份，供组件自己的离场输入持有。
    pub(crate) fn inline_image(self, url: &MediaUrl, area: Rect) -> Option<InlineImage> {
        self.engine.inline_image(url, area)
    }

    /// 已保留图片是否仍属于当前终端协议代次。
    pub(crate) fn inline_is_current(self, image: &InlineImage) -> bool {
        self.engine.inline_is_current(image)
    }
}

/// 一次准备所声明的图片需求；声明本身不启动工作。
pub(crate) struct ImageNeeds<'a> {
    /// 当前已就绪资源，仅用于选择尺寸和保留过渡输入。
    ready: ReadyImages<'a>,

    /// 布局确定后交回管线协调的需求。
    requests: Vec<ImageRequest>,
}

/// 由组件布局产生的资源操作，不包含组件种类或使用位置。
pub(crate) enum ImageRequest {
    /// 显示一个封面区域。
    Display {
        /// 图片身份；无图仍保留采样几何。
        url: Option<MediaUrl>,

        /// 布局分配的区域。
        area: Rect,

        /// 区域当前的稳定程度。
        phase: ImageRenderPhase,
    },

    /// 按稳定尺寸预热已解码的图片。
    Prewarm(MediaUrl, Rect),

    /// 过渡仍在使用的源图片。
    Visible(MediaUrl),

    /// 一行可见封面，缺 URL 时仍声明低清采样尺寸。
    Thumbnail {
        /// 封面身份。
        url: Option<MediaUrl>,

        /// 终端字符格区域。
        area: Rect,

        /// 当前几何与滚动阶段。
        phase: ImageRenderPhase,
    },

    /// 仍参与过渡的已就绪图片。
    Retain(InlineImage),
}

impl<'a> ImageNeeds<'a> {
    /// 从本次资源快照开始收集，绘制次数不参与收集。
    pub(crate) fn new(ready: ReadyImages<'a>) -> Self {
        Self {
            ready,
            requests: Vec::new(),
        }
    }

    /// 只读借出当前成品，不能改变资源生命周期。
    pub(crate) fn ready(&self) -> ReadyImages<'a> {
        self.ready
    }

    /// 声明本次布局中的封面。
    pub(crate) fn display(&mut self, url: Option<&MediaUrl>, area: Rect, phase: ImageRenderPhase) {
        self.requests.push(ImageRequest::Display {
            url: url.cloned(),
            area,
            phase,
        });
    }

    /// 声明即将使用的稳定尺寸。
    pub(crate) fn prewarm(&mut self, url: &MediaUrl, area: Rect) {
        self.requests.push(ImageRequest::Prewarm(url.clone(), area));
    }

    /// 声明过渡仍需要该源图片。
    pub(crate) fn visible(&mut self, url: &MediaUrl) {
        self.requests.push(ImageRequest::Visible(url.clone()));
    }

    /// 声明一个可见缩略图。
    pub(crate) fn thumbnail(
        &mut self,
        url: Option<&MediaUrl>,
        area: Rect,
        phase: ImageRenderPhase,
    ) {
        self.requests.push(ImageRequest::Thumbnail {
            url: url.cloned(),
            area,
            phase,
        });
    }

    /// 声明一个过渡期间仍在使用的图片身份。
    pub(crate) fn retain(&mut self, image: &InlineImage) {
        self.requests.push(ImageRequest::Retain(image.clone()));
    }

    /// 结束只读借用，把需求交回图片管线。
    pub(crate) fn finish(self) -> Vec<ImageRequest> {
        self.requests
    }
}

impl ImageEngine {
    /// 借给绘制和布局的只读资源接口。
    pub(crate) fn ready(&self) -> ReadyImages<'_> {
        ReadyImages { engine: self }
    }

    /// 布局准备结束后统一协调资源；该入口独占管线的可变借用。
    pub(crate) fn reconcile(&mut self, requests: impl IntoIterator<Item = ImageRequest>) {
        for request in requests {
            match request {
                ImageRequest::Display { url, area, phase } => {
                    self.prepare_display(ImageContent::Display { url: url.as_ref() }, area, phase)
                }
                ImageRequest::Prewarm(url, area) => self.prepare(&url, area),
                ImageRequest::Visible(url) => self.observe_visible(&url),
                ImageRequest::Thumbnail { url, area, phase } => {
                    self.prepare_thumbnail(url.as_ref(), area, phase)
                }
                ImageRequest::Retain(image) => self.retain_inline(&image),
            }
        }
    }
}

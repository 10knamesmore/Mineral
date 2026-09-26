//! 根据图片内容和渲染阶段统一选择终端成品或 halfblock。
//!
//! 稳定区域可以复用缓存的终端成品；区域逐帧变化或离屏合成只使用纯 cell
//! halfblock。终端成品未就绪时当前帧仍显示低清图片，并在允许的阶段投递后台编码；
//! 完整源图片未就绪时优先显示真实低清 preview；preview 也未就绪时保留调用方背景。

use std::sync::Arc;

use image::{DynamicImage, Rgba, RgbaImage};
use mineral_model::MediaUrl;
use mineral_protocol::AdvanceKind;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::image::encode::EncodeRequest;
use crate::render::color::lerp_byte;

use super::ImageEngine;
use super::geometry::{fitted_area, square_subarea};
use super::graphics::GraphicsProtocol;
use super::key::{ImageIdentity, PixelSize, TerminalImageKey};
use super::terminal::{render_pixels, sample_pixels};

/// 双图合成方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlendStyle {
    /// 按透明度交叉淡入淡出。
    Fade,

    /// 旧图左移退场，新图从右侧进入。
    Slide,

    /// 旧图放大退场，新图缩小落定。
    Zoom,
}

impl From<mineral_config::CoverTransitionStyle> for BlendStyle {
    fn from(value: mineral_config::CoverTransitionStyle) -> Self {
        match value {
            mineral_config::CoverTransitionStyle::Slide => Self::Slide,
            mineral_config::CoverTransitionStyle::Zoom => Self::Zoom,
            _ => Self::Fade,
        }
    }
}

/// 一次图片渲染所需的内容。
#[derive(Clone, Copy)]
pub(crate) enum ImageContent<'a> {
    /// 显示单张 URL 图片；完整图片未解码时显示 preview，preview 也未就绪时不绘制。
    Display {
        /// 真实图片 URL。
        url: Option<&'a MediaUrl>,
    },

    /// 把两张 URL 图片合成为一帧。
    Blend {
        /// 退场图片。
        from: &'a MediaUrl,

        /// 进场图片。
        to: &'a MediaUrl,

        /// 合成进度，范围 `0..=1000`。
        progress: u16,

        /// 合成方式。
        style: BlendStyle,

        /// 进入档位；`Prev` 反转向,其余按「下一首」。
        advance: Option<AdvanceKind>,
    },
}

/// 一帧双图合成所需的完整输入。
#[derive(Clone, Copy)]
struct BlendContent<'a> {
    /// 退场图片。
    from: &'a MediaUrl,

    /// 进场图片。
    to: &'a MediaUrl,

    /// 合成进度，范围 `0..=1000`。
    progress: u16,

    /// 合成方式。
    style: BlendStyle,

    /// 进入档位；`Prev` 反转向,其余按「下一首」。
    advance: Option<AdvanceKind>,
}

/// 决定终端图片是否可以安全复用的互斥渲染阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageRenderPhase {
    /// 屏上区域与内容均已稳定，可以使用终端图片成品。
    Stable,

    /// 列表仍在滚动，可以显示已有成品但不提交新的昂贵编码任务。
    Scrolling,

    /// 图片区域逐帧变化；大图使用 halfblock，行内缩略图复用可逐格搬运的 Kitty 成品。
    Resizing,

    /// 渲染到离屏 cell buffer；大图使用 halfblock，行内缩略图复用纯 Unicode 占位字符。
    Offscreen,
}

impl ImageEngine {
    /// 按图片内容与渲染阶段完成本帧绘制。
    ///
    /// # Params:
    ///   - `content`: 单图或双图合成内容
    ///   - `area`: 调用方提供的 cell 区域
    ///   - `buf`: 当前屏幕或离屏缓冲
    ///   - `phase`: 当前互斥渲染阶段
    pub(crate) fn render(
        &self,
        content: ImageContent<'_>,
        area: Rect,
        buf: &mut Buffer,
        phase: ImageRenderPhase,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        match content {
            ImageContent::Display { url } => self.render_display(url, area, buf, phase),
            ImageContent::Blend {
                from,
                to,
                progress,
                style,
                advance,
            } => self.render_blend(
                BlendContent {
                    from,
                    to,
                    progress,
                    style,
                    advance,
                },
                area,
                buf,
                phase,
            ),
        }
    }

    /// 提前准备 URL 图片在稳定区域使用的终端成品。
    pub(crate) fn prepare(&self, url: &MediaUrl, area: Rect) {
        let target = square_subarea(area, self.cell_pixels());
        let Some(image) = self.cache.get(url).cloned() else {
            self.demand_decode(url);
            return;
        };
        let target = fitted_area(target, &image, self.cell_pixels());
        self.prepare_image(ImageIdentity::Url(url.clone()), image, target);
    }

    /// 两张已解码封面是否同一张图(内容指纹比对)。任一未解码时为 `false`。
    ///
    /// # Params:
    ///   - `left` / `right`: 待比较的两个封面 URL
    pub(crate) fn same_picture(&self, left: &MediaUrl, right: &MediaUrl) -> bool {
        self.cache.same_picture(left, right)
    }

    /// 把一个 URL 登记进本帧可见工作集，预算逐出会跳过它们。
    ///
    /// # Params:
    ///   - `url`: 本帧确实参与展示的封面
    pub(crate) fn observe_visible(&self, url: &MediaUrl) {
        self.cache.observe_visible(url);
    }

    /// 返回需要避开背景重绘的协议覆盖区；Kitty 和 halfblock 保留逐格动态背景。
    pub(crate) fn ready_area(&self, url: &MediaUrl, area: Rect) -> Option<Rect> {
        if matches!(
            self.graphics_protocol(),
            GraphicsProtocol::Kitty | GraphicsProtocol::Halfblocks
        ) {
            return None;
        }
        let image = self.cache.get(url)?;
        let target = fitted_area(
            square_subarea(area, self.cell_pixels()),
            image,
            self.cell_pixels(),
        );
        let key = self.terminal_key(ImageIdentity::Url(url.clone()), target);
        self.terminal_images.ready(&key).then_some(target)
    }

    /// 把稳定区域转换为视觉正方区域。
    pub(crate) fn square_area(&self, area: Rect) -> Rect {
        square_subarea(area, self.cell_pixels())
    }

    /// 显示单图；完整图片未解码时优先显示 preview，否则保留现有背景。
    fn render_display(
        &self,
        url: Option<&MediaUrl>,
        area: Rect,
        buf: &mut Buffer,
        phase: ImageRenderPhase,
    ) {
        if let Some(url) = url {
            self.cache.observe_visible(url);
        }
        let target = square_subarea(area, self.cell_pixels());
        if matches!(
            phase,
            ImageRenderPhase::Stable | ImageRenderPhase::Scrolling
        ) {
            self.observe_preview_target(target);
        }
        if phase == ImageRenderPhase::Stable
            && let Some(url) = url
        {
            self.demand_decode(url);
        }
        let Some((identity, image)) = self.resolve_display(url) else {
            if let Some(url) = url {
                let key = self.preview_key(url, target);
                let _ = self.preview_images.render_if_ready(&key, |preview| {
                    let _ = preview.render(target, buf);
                });
            }
            return;
        };
        let target = fitted_area(target, &image, self.cell_pixels());
        if matches!(
            phase,
            ImageRenderPhase::Resizing | ImageRenderPhase::Offscreen
        ) {
            render_halfblock_to(buf, target, &image, self.cell_pixels());
            return;
        }
        let key = self.terminal_key(identity.clone(), target);
        let rendered = self
            .terminal_images
            .render_if_ready(&key, |terminal_image| {
                if let Some(command) = terminal_image.render(target, buf) {
                    self.graphics_commands.borrow_mut().push_str(&command);
                }
            });
        if rendered {
            return;
        }
        render_halfblock_to(buf, target, &image, self.cell_pixels());
        if phase == ImageRenderPhase::Stable {
            self.prepare_image(identity, image, target);
        }
    }

    /// 合成两张已解码图片；任一未就绪时尝试显示进场图片。
    fn render_blend(
        &self,
        content: BlendContent<'_>,
        area: Rect,
        buf: &mut Buffer,
        phase: ImageRenderPhase,
    ) {
        self.cache.observe_visible(content.from);
        self.cache.observe_visible(content.to);
        let (Some(from_image), Some(to_image)) =
            (self.cache.get(content.from), self.cache.get(content.to))
        else {
            self.render_display(Some(content.to), area, buf, phase);
            return;
        };
        let target = match phase {
            ImageRenderPhase::Resizing => area,
            ImageRenderPhase::Offscreen
            | ImageRenderPhase::Stable
            | ImageRenderPhase::Scrolling => square_subarea(area, self.cell_pixels()),
        };
        let composite = compose_transition(BlendFrame {
            from: from_image,
            to: to_image,
            px_w: u32::from(target.width),
            px_h: u32::from(target.height).saturating_mul(2),
            cell_pixels: self.cell_pixels(),
            style: content.style,
            progress_permille: content.progress,
            advance: content.advance,
            zoom_scale_permille: permille_of_scale(self.transition_zoom_scale()),
        });
        render_pixels(&composite, target, buf);
    }

    /// 返回已经解码的真实图片。
    fn resolve_display(
        &self,
        url: Option<&MediaUrl>,
    ) -> Option<(ImageIdentity, Arc<DynamicImage>)> {
        if let Some(url) = url
            && let Some(image) = self.cache.get(url).cloned()
        {
            return Some((ImageIdentity::Url(url.clone()), image));
        }
        None
    }

    /// 按当前 backend 构造源图片键或 rasterized 像素键。
    fn terminal_key(&self, identity: ImageIdentity, target: Rect) -> TerminalImageKey {
        if self.graphics_protocol() == GraphicsProtocol::Kitty {
            TerminalImageKey::source(identity)
        } else {
            TerminalImageKey::rasterized(
                identity,
                PixelSize::from_cells((target.width, target.height), self.cell_pixels()),
            )
        }
    }

    /// 去重并投递一个终端图片编码任务。
    fn prepare_image(&self, identity: ImageIdentity, image: Arc<DynamicImage>, target: Rect) {
        if target.width == 0 || target.height == 0 {
            return;
        }
        let key = self.terminal_key(identity, target);
        if self.terminal_images.contains(&key) {
            return;
        }
        if self.encode_pending.borrow_mut().insert(key.clone()) {
            mineral_log::debug!(target: "cover", source_width = image.width(), source_height = image.height(),
                cells = ?(target.width, target.height), cell_pixels = ?self.cell_pixels(),
                protocol = ?self.graphics_protocol(), "prepare fitted cover image");
            self.request_encode(EncodeRequest {
                key,
                generation: self.graphics_generation(),
                image,
                target,
            });
        }
    }
}

/// 一帧双图合成的像素级输入:两图已解码、输出像素网格已定。
#[derive(Clone, Copy)]
struct BlendFrame<'a> {
    /// 退场图。
    from: &'a DynamicImage,

    /// 进场图。
    to: &'a DynamicImage,

    /// 输出像素网格宽(cell 列数)。
    px_w: u32,

    /// 输出像素网格高(cell 行数 × 2)。
    px_h: u32,

    /// 终端 cell 的真实像素宽高，动画与协议图使用相同比例。
    cell_pixels: (u16, u16),

    /// 合成方式。
    style: BlendStyle,

    /// 合成进度,范围 `0..=1000`(已缓动)。
    progress_permille: u16,

    /// 进入档位；`Prev` 反转向,其余按「下一首」。
    advance: Option<AdvanceKind>,

    /// zoom 样式的缩放幅度(‰,`permille_of_scale` 折算)。
    zoom_scale_permille: u32,
}

/// 把新旧两张图片按样式与进度合成一帧 halfblock 像素图。
///
/// # Params:
///   - `frame`: 一帧合成的完整像素级输入
fn compose_transition(frame: BlendFrame<'_>) -> RgbaImage {
    let BlendFrame {
        from,
        to,
        px_w,
        px_h,
        cell_pixels,
        style,
        progress_permille,
        advance,
        zoom_scale_permille,
    } = frame;
    let cells = (
        u16::try_from(px_w).unwrap_or(u16::MAX),
        u16::try_from(px_h / 2).unwrap_or(u16::MAX),
    );
    let old = sample_pixels(from, cells, cell_pixels);
    let new = sample_pixels(to, cells, cell_pixels);
    let p = u64::from(progress_permille.min(1000));
    match style {
        BlendStyle::Slide => RgbaImage::from_fn(px_w, px_h, |x, y| {
            let shift = u32::try_from(u64::from(px_w).saturating_mul(p) / 1000).unwrap_or(0);
            match advance {
                Some(AdvanceKind::Prev) => {
                    if x >= shift {
                        pixel_at(&old, x - shift, y)
                    } else {
                        pixel_at(&new, x + px_w - shift, y)
                    }
                }
                Some(AdvanceKind::Next | AdvanceKind::RandomAccess) | None => {
                    let shifted = x.saturating_add(shift);
                    if shifted < px_w {
                        pixel_at(&old, shifted, y)
                    } else {
                        pixel_at(&new, shifted - px_w, y)
                    }
                }
            }
        }),
        BlendStyle::Zoom => {
            // 旧图从静止尺寸出发、新图落定回静止尺寸,透明度随进度交叉。
            let (old_scale, new_scale) =
                zoom_scales(advance, progress_permille, zoom_scale_permille);
            RgbaImage::from_fn(px_w, px_h, |x, y| {
                blend_pixel(
                    sample_zoomed(&old, x, y, old_scale),
                    sample_zoomed(&new, x, y, new_scale),
                    p,
                )
            })
        }
        BlendStyle::Fade => RgbaImage::from_fn(px_w, px_h, |x, y| {
            blend_pixel(pixel_at(&old, x, y), pixel_at(&new, x, y), p)
        }),
    }
}

/// [`BlendStyle::Zoom`] 在进度 `progress_permille`(‰)下给退场图 / 进场图的采样缩放(千分比)。
///
/// 两端都锚在 1000(静止尺寸),收尾不跳变。`Next` 迎面推近(退场图放大到
/// `zoom_scale_permille`、进场图从那里回缩落定),`Prev` 反向退远(退场图缩小到倒数、
/// 进场图从那里推进落定)。
///
/// # Params:
///   - `advance`: 进入档位;`Prev` 反转向,其余都按 `Next` 算
///   - `progress_permille`: 转场进度(‰)
///   - `zoom_scale_permille`: 缩放幅度(≥ 1000;1000 = 无缩放)
///
/// # Return:
///   `(退场图缩放, 进场图缩放)`。
fn zoom_scales(
    advance: Option<AdvanceKind>,
    progress_permille: u16,
    zoom_scale_permille: u32,
) -> (u32, u32) {
    let p = u64::from(progress_permille.min(1000));
    let scale = zoom_scale_permille.max(1000);
    let span = u64::from(scale.saturating_sub(1000));
    match advance {
        Some(AdvanceKind::Prev) => {
            // 远端 = 静止尺寸 / 缩放幅度(千分比倒数,整数取整);退场图 1000 → 远端,
            // 进场图远端 → 1000。
            let far = 1_000_000 / scale;
            let travel =
                u32::try_from(u64::from(1000_u32.saturating_sub(far)) * p / 1000).unwrap_or(0);
            (1000_u32.saturating_sub(travel), far.saturating_add(travel))
        }
        Some(AdvanceKind::Next | AdvanceKind::RandomAccess) | None => {
            let step = u32::try_from(span * p / 1000).unwrap_or(0);
            (1000_u32.saturating_add(step), scale.saturating_sub(step))
        }
    }
}

/// 先按 alpha 加权再交叉渐变，避免图片与透明留白之间出现黑边。
fn blend_pixel(old: Rgba<u8>, new: Rgba<u8>, progress: u64) -> Rgba<u8> {
    let Rgba([old_r, old_g, old_b, old_a]) = old;
    let Rgba([new_r, new_g, new_b, new_a]) = new;
    let old_weight = u64::from(old_a) * (1000 - progress);
    let new_weight = u64::from(new_a) * progress;
    let weight = old_weight + new_weight;
    if weight == 0 {
        return Rgba([0, 0, 0, 0]);
    }
    let channel = |a: u8, b: u8| lerp_byte(a, b, new_weight, weight);
    Rgba([
        channel(old_r, new_r),
        channel(old_g, new_g),
        channel(old_b, new_b),
        lerp_byte(old_a, new_a, progress, 1000),
    ])
}

/// 画布以外是透明背景，不复制边缘像素来填满空隙。
fn pixel_at(img: &RgbaImage, x: u32, y: u32) -> Rgba<u8> {
    img.get_pixel_checked(x, y)
        .copied()
        .unwrap_or(Rgba([0, 0, 0, 0]))
}

/// 以图心等比缩放；缩小后的画布外保留透明背景。
fn sample_zoomed(img: &RgbaImage, x: u32, y: u32, scale_permille: u32) -> Rgba<u8> {
    let scale = i64::from(scale_permille.max(1));
    let map = |v: u32, dim: u32| -> Option<u32> {
        let center = i64::from(dim) * 500;
        let src = center + (i64::from(v) * 1000 + 500 - center) * 1000 / scale;
        (src >= 0 && src < i64::from(dim) * 1000)
            .then(|| u32::try_from(src / 1000).ok())
            .flatten()
    };
    match (map(x, img.width()), map(y, img.height())) {
        (Some(x), Some(y)) => pixel_at(img, x, y),
        _ => Rgba([0, 0, 0, 0]),
    }
}

/// 缩放倍数 → 千分比定点(clamp 进转场缩放的合理域再转)。
#[allow(clippy::as_conversions)] // reason: 已 clamp 进 1.0..=4.0 且 round,转换语义无损
fn permille_of_scale(scale: f32) -> u32 {
    (scale.clamp(1.0, 4.0) * 1000.0).round() as u32
}

/// 按真实终端像素比例完整显示封面；透明留白与本帧背景合成。
fn render_halfblock_to(
    buf: &mut Buffer,
    area: Rect,
    image: &DynamicImage,
    cell_pixels: (u16, u16),
) {
    if area.is_empty() {
        return;
    }
    let pixels = sample_pixels(image, (area.width, area.height), cell_pixels);
    render_pixels(&pixels, area, buf);
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use image::{DynamicImage, Rgb, RgbImage};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use super::render_halfblock_to;

    /// Kitty 图片已经就绪时，留白背景仍逐格更新，而且不再次传输或创建 placement。
    #[test]
    fn kitty_cover_background_updates_without_retransmission() -> color_eyre::Result<()> {
        use crate::image::graphics::TerminalGraphics;
        use crate::image::key::ImageIdentity;
        use crate::image::terminal::TerminalImage;
        use crate::image::{ImageContent, ImageEngine, ImageRenderPhase};
        use mineral_model::MediaUrl;
        use std::sync::Arc;
        let mut engine = ImageEngine::disabled_kitty(Arc::new(mineral_config::Config::defaults()?));
        let url = MediaUrl::remote("https://example.com/kitty-wide.png")?;
        let image = Arc::new(DynamicImage::ImageRgb8(RgbImage::from_pixel(
            301,
            199,
            Rgb([220, 40, 60]),
        )));
        engine.cache.insert_test(&url, Arc::clone(&image));
        let area = Rect::new(2, 2, 32, 16);
        let target = super::fitted_area(area, &image, engine.cell_pixels());
        let encoded = TerminalImage::encode(
            &image,
            None,
            (target.width, target.height),
            &TerminalGraphics::fixed_kitty(engine.cell_pixels()),
        )?;
        let bytes = encoded.resident_bytes();
        engine.terminal_images.insert(
            &engine.terminal_key(ImageIdentity::Url(url.clone()), target),
            encoded,
            bytes,
        );
        assert_eq!(
            engine.ready_area(&url, area),
            None,
            "Kitty cannot exclude the cover frame from ambient rendering"
        );
        let mut previous = Buffer::empty(area);
        for step in [0_u8, 1] {
            let mut buffer = Buffer::empty(area);
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    buffer
                        .cell_mut((x, y))
                        .ok_or_else(|| eyre!("missing cell"))?
                        .set_bg(Color::Rgb(u8::try_from(x)?, u8::try_from(y)?, step * 100));
                }
            }
            engine.render(
                ImageContent::Display { url: Some(&url) },
                area,
                &mut buffer,
                ImageRenderPhase::Stable,
            );
            for y in area.top()..area.bottom() {
                for x in area.left()..area.right() {
                    let cell = buffer.cell((x, y)).ok_or_else(|| eyre!("missing cell"))?;
                    assert!(!cell.skip);
                    assert!(!cell.symbol().contains('\x1b'));
                    assert_eq!(
                        cell.bg,
                        Color::Rgb(u8::try_from(x)?, u8::try_from(y)?, step * 100)
                    );
                }
            }
            if step == 0 {
                let commands = std::mem::take(&mut *engine.graphics_commands.borrow_mut());
                assert!(commands.contains("a=t"));
                assert!(commands.contains("a=p"));
            } else {
                assert!(engine.graphics_commands.borrow().is_empty());
                assert_eq!(
                    previous.diff(&buffer).len(),
                    usize::from(area.width) * usize::from(area.height)
                );
            }
            previous = buffer;
        }
        Ok(())
    }

    /// 纯色图降采样:每个 cell 都是 `▀`,fg/bg 同为该色 —— 均匀图无边缘,Triangle 重采样不改色,
    /// 故期望色可精确断言。
    #[test]
    fn halfblock_uniform_image_fills_solid() -> color_eyre::Result<()> {
        let mut img = RgbImage::new(8, 8);
        for p in img.pixels_mut() {
            *p = Rgb([200, 50, 50]);
        }
        let image = DynamicImage::ImageRgb8(img);
        let area = Rect::new(0, 0, 4, 2);
        let mut buf = Buffer::empty(area);

        render_halfblock_to(&mut buf, area, &image, (8, 16));

        for y in 0..2u16 {
            for x in 0..4u16 {
                let cell = buf
                    .cell((x, y))
                    .ok_or_else(|| eyre!("cell ({x},{y}) 越界"))?;
                assert_eq!(cell.symbol(), "▀", "cell ({x},{y}) 应为上半字符");
                assert_eq!(
                    cell.fg,
                    Color::Rgb(200, 50, 50),
                    "cell ({x},{y}) 上半像素色"
                );
                assert_eq!(
                    cell.bg,
                    Color::Rgb(200, 50, 50),
                    "cell ({x},{y}) 下半像素色"
                );
            }
        }
        Ok(())
    }

    /// 上半红 / 下半蓝：顶 cell 取顶部像素，底 cell 取底部像素，证明采样来自输入图。
    /// 中间 cell 跨红蓝边界会混色，只断言远离边界的顶 / 底 cell。
    #[test]
    fn halfblock_samples_top_and_bottom() -> color_eyre::Result<()> {
        let mut img = RgbImage::new(4, 16);
        for (_x, y, p) in img.enumerate_pixels_mut() {
            *p = if y < 8 {
                Rgb([220, 0, 0])
            } else {
                Rgb([0, 0, 220])
            };
        }
        let image = DynamicImage::ImageRgb8(img);
        let area = Rect::new(0, 0, 4, 4);
        let mut buf = Buffer::empty(area);

        render_halfblock_to(&mut buf, area, &image, (4, 16));

        let top = buf.cell((0, 0)).ok_or_else(|| eyre!("顶 cell 越界"))?;
        assert_eq!(top.fg, Color::Rgb(220, 0, 0), "顶 cell 上半 = 红");
        let bottom = buf.cell((0, 3)).ok_or_else(|| eyre!("底 cell 越界"))?;
        assert_eq!(bottom.bg, Color::Rgb(0, 0, 220), "底 cell 下半 = 蓝");
        Ok(())
    }
}

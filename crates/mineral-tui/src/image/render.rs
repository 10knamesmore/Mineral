//! 根据图片内容和渲染阶段统一选择终端成品、halfblock 或随机 fallback。
//!
//! 稳定区域可以复用缓存的终端成品；区域逐帧变化或离屏合成只使用纯 cell
//! halfblock。终端成品未就绪时当前帧仍显示低清图片，并在允许的阶段投递后台编码。

use std::sync::Arc;

use image::{DynamicImage, Rgb, RgbImage};
use mineral_model::MediaUrl;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};

use crate::image::encode::EncodeRequest;
use crate::render::color::lerp_byte;

use super::geometry::{square_cells, square_subarea};
use super::graphics::GraphicsProtocol;
use super::key::{ImageIdentity, PixelSize, TerminalImageKey};
use super::{ImageEngine, fallback};

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
    /// 显示单张 URL 图片；真实图片未解码时显示随机 fallback。
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
}

/// 决定终端图片是否可以安全复用的互斥渲染阶段。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImageRenderPhase {
    /// 屏上区域与内容均已稳定，可以使用终端图片成品。
    Stable,

    /// 列表仍在滚动，可以显示已有成品但不提交新的昂贵编码任务。
    Scrolling,

    /// 图片区域逐帧变化，只使用不持有终端 image id 的 halfblock。
    Resizing,

    /// 渲染到离屏 cell buffer，只使用 halfblock。
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
    ///   - `theme`: 随机 fallback 使用的当前主题
    pub(crate) fn render(
        &self,
        content: ImageContent<'_>,
        area: Rect,
        buf: &mut Buffer,
        phase: ImageRenderPhase,
        theme: &crate::render::theme::Theme,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        match content {
            ImageContent::Display { url } => self.render_display(url, area, buf, phase, theme),
            ImageContent::Blend {
                from,
                to,
                progress,
                style,
            } => self.render_blend(
                BlendContent {
                    from,
                    to,
                    progress,
                    style,
                },
                area,
                buf,
                phase,
                theme,
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
        self.prepare_image(ImageIdentity::Url(url.clone()), image, target);
    }

    /// 返回 URL 图片已经可用的终端成品区域。
    pub(crate) fn ready_area(&self, url: &MediaUrl, area: Rect) -> Option<Rect> {
        let target = square_subarea(area, self.cell_pixels());
        let key = self.terminal_key(ImageIdentity::Url(url.clone()), target);
        self.terminal_images.ready(&key).then_some(target)
    }

    /// 把稳定区域转换为视觉正方区域。
    pub(crate) fn square_area(&self, area: Rect) -> Rect {
        square_subarea(area, self.cell_pixels())
    }

    /// 显示单图；真实图片未解码时直接绘制随机 fallback。
    fn render_display(
        &self,
        url: Option<&MediaUrl>,
        area: Rect,
        buf: &mut Buffer,
        phase: ImageRenderPhase,
        theme: &crate::render::theme::Theme,
    ) {
        let target = match phase {
            ImageRenderPhase::Offscreen => square_cells(area),
            ImageRenderPhase::Stable | ImageRenderPhase::Scrolling | ImageRenderPhase::Resizing => {
                square_subarea(area, self.cell_pixels())
            }
        };
        if phase == ImageRenderPhase::Stable
            && let Some(url) = url
        {
            self.demand_decode(url);
        }
        let Some((identity, image)) = self.resolve_display(url) else {
            fallback::render_random(buf, target, theme);
            return;
        };
        if matches!(
            phase,
            ImageRenderPhase::Resizing | ImageRenderPhase::Offscreen
        ) {
            render_halfblock_to(buf, target, &image);
            return;
        }
        let key = self.terminal_key(identity.clone(), target);
        let rendered = self
            .terminal_images
            .render_if_ready(&key, |terminal_image| terminal_image.render(target, buf));
        if rendered {
            return;
        }
        render_halfblock_to(buf, target, &image);
        if phase == ImageRenderPhase::Stable {
            self.prepare_image(identity, image, target);
        }
    }

    /// 合成两张已解码图片；任一未就绪时显示进场图片或随机 fallback。
    fn render_blend(
        &self,
        content: BlendContent<'_>,
        area: Rect,
        buf: &mut Buffer,
        phase: ImageRenderPhase,
        theme: &crate::render::theme::Theme,
    ) {
        let (Some(from_image), Some(to_image)) =
            (self.cache.get(content.from), self.cache.get(content.to))
        else {
            self.render_display(Some(content.to), area, buf, phase, theme);
            return;
        };
        let target = match phase {
            ImageRenderPhase::Resizing => area,
            ImageRenderPhase::Offscreen => square_cells(area),
            ImageRenderPhase::Stable | ImageRenderPhase::Scrolling => {
                square_subarea(area, self.cell_pixels())
            }
        };
        let composite = compose_transition(
            from_image,
            to_image,
            u32::from(target.width),
            u32::from(target.height).saturating_mul(2),
            content.style,
            content.progress,
            permille_of_scale(self.transition_zoom_scale()),
        );
        render_halfblock_to(buf, target, &DynamicImage::ImageRgb8(composite));
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
            self.request_encode(EncodeRequest {
                key,
                generation: self.graphics_generation(),
                image,
                target,
            });
        }
    }
}

/// 把新旧两张图片按样式与进度合成一帧 halfblock 像素图。
///
/// # Params:
///   - `px_w` / `px_h`: 输出像素网格(宽 = cell 列数,高 = cell 行数 × 2)
///   - `progress_permille`: 转场进度(‰,已缓动)
///   - `zoom_scale_permille`: zoom 样式的缩放幅度(‰,`permille_of_scale` 折算)
fn compose_transition(
    from: &DynamicImage,
    to: &DynamicImage,
    px_w: u32,
    px_h: u32,
    style: BlendStyle,
    progress_permille: u16,
    zoom_scale_permille: u32,
) -> RgbImage {
    let old = from
        .resize_exact(px_w, px_h, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let new = to
        .resize_exact(px_w, px_h, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let p = u64::from(progress_permille.min(1000));
    match style {
        BlendStyle::Slide => RgbImage::from_fn(px_w, px_h, |x, y| {
            // 旧图整体左移 p·w 退场,新图贴着旧图右缘推入。
            let shifted = x.saturating_add(
                u32::try_from(u64::from(px_w).saturating_mul(p) / 1000).unwrap_or(0),
            );
            if shifted < px_w {
                pixel_at(&old, shifted, y)
            } else {
                pixel_at(&new, shifted - px_w, y)
            }
        }),
        BlendStyle::Zoom => {
            // 旧图 1 → scale 放大退场,新图 scale → 1 回缩落定,透明度随进度交叉。
            let step =
                u32::try_from(u64::from(zoom_scale_permille.saturating_sub(1000)) * p / 1000)
                    .unwrap_or(0);
            let grow = 1000_u32.saturating_add(step);
            let shrink = zoom_scale_permille.saturating_sub(step).max(1000);
            RgbImage::from_fn(px_w, px_h, |x, y| {
                let Rgb([old_r, old_g, old_b]) = sample_zoomed(&old, x, y, grow);
                let Rgb([new_r, new_g, new_b]) = sample_zoomed(&new, x, y, shrink);
                Rgb([
                    lerp_byte(old_r, new_r, p, 1000),
                    lerp_byte(old_g, new_g, p, 1000),
                    lerp_byte(old_b, new_b, p, 1000),
                ])
            })
        }
        BlendStyle::Fade => RgbImage::from_fn(px_w, px_h, |x, y| {
            let Rgb([old_r, old_g, old_b]) = pixel_at(&old, x, y);
            let Rgb([new_r, new_g, new_b]) = pixel_at(&new, x, y);
            Rgb([
                lerp_byte(old_r, new_r, p, 1000),
                lerp_byte(old_g, new_g, p, 1000),
                lerp_byte(old_b, new_b, p, 1000),
            ])
        }),
    }
}

/// 越界安全取像素(合成坐标域与图同构,黑色 fallback 仅兜类型穷尽)。
fn pixel_at(img: &RgbImage, x: u32, y: u32) -> Rgb<u8> {
    img.get_pixel_checked(x, y)
        .copied()
        .unwrap_or(Rgb([0, 0, 0]))
}

/// 以图心为原点按千分比缩放采样:输出坐标映射回源坐标 `c + (v - c)·1000 / scale`。
/// `scale ≥ 1000` 时采样窗内收不出界,仍 clamp 兜边。
fn sample_zoomed(img: &RgbImage, x: u32, y: u32, scale_permille: u32) -> Rgb<u8> {
    let scale = i64::from(scale_permille.max(1));
    let map = |v: u32, dim: u32| -> u32 {
        let center = i64::from(dim).saturating_mul(500);
        let v_permille = i64::from(v).saturating_mul(1000).saturating_add(500);
        let src = center + (v_permille - center) * 1000 / scale;
        u32::try_from((src / 1000).clamp(0, i64::from(dim.saturating_sub(1)))).unwrap_or(0)
    };
    pixel_at(img, map(x, img.width()), map(y, img.height()))
}

/// 缩放倍数 → 千分比定点(clamp 进转场缩放的合理域再转)。
#[allow(clippy::as_conversions)] // reason: 已 clamp 进 1.0..=4.0 且 round,转换语义无损
fn permille_of_scale(scale: f32) -> u32 {
    (scale.clamp(1.0, 4.0) * 1000.0).round() as u32
}

/// 把已解码图片按 halfblock(`▀`)逐 cell 画进 `area`。
///
/// 正方区由调用方算好再传入)。每 cell:上半像素 → fg、下半像素 → bg;源图先 `resize_exact`
/// 到 `area.width × area.height*2` 像素再逐 cell 采样。
///
/// 纯写终端 cell、不持有终端 image id 或协议缓冲，区域逐帧变化时可以安全重画。
/// 降采样在渲染线程同步做:源图 ≤ 384px、目标几十像素,Triangle 一次亚毫秒级。
///
/// # Params:
///   - `buf`: 目标缓冲(屏上 / 离屏皆可)
///   - `area`: 铺图区域(宽高任一为 0 直接返回)
///   - `image`: 已解码封面原图
pub(crate) fn render_halfblock_to(buf: &mut Buffer, area: Rect, image: &DynamicImage) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let px_w = u32::from(area.width);
    let px_h = u32::from(area.height).saturating_mul(2);
    let small = image
        .resize_exact(px_w, px_h, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let sample = |x: u32, y: u32| -> Color {
        small.get_pixel_checked(x, y).map_or(Color::Reset, |p| {
            let Rgb([r, g, b]) = *p;
            Color::Rgb(r, g, b)
        })
    };
    for cy in 0..area.height {
        let py = u32::from(cy).saturating_mul(2);
        for cx in 0..area.width {
            let px = u32::from(cx);
            let style = Style::new()
                .fg(sample(px, py))
                .bg(sample(px, py.saturating_add(1)));
            buf.set_string(area.x + cx, area.y + cy, "▀", style);
        }
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use image::{DynamicImage, Rgb, RgbImage};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Color;

    use super::render_halfblock_to;

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

        render_halfblock_to(&mut buf, area, &image);

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

    use super::{BlendStyle, compose_transition};
    use image::Rgb as PxRgb;

    /// 造一张纯色图。
    fn solid(r: u8, g: u8, b: u8) -> DynamicImage {
        let mut img = RgbImage::new(16, 16);
        for p in img.pixels_mut() {
            *p = PxRgb([r, g, b]);
        }
        DynamicImage::ImageRgb8(img)
    }

    /// fade 中点:纯红 → 纯蓝在 500‰ 处逐像素恰为均值(整数 lerp 无偏差)。
    #[test]
    fn compose_fade_midpoint_is_average() -> color_eyre::Result<()> {
        let out = compose_transition(
            &solid(200, 0, 0),
            &solid(0, 0, 200),
            /*px_w*/ 8,
            /*px_h*/ 8,
            BlendStyle::Fade,
            /*progress_permille*/ 500,
            /*zoom_scale_permille*/ 1120,
        );
        for p in out.pixels() {
            assert_eq!(*p, PxRgb([100, 0, 100]), "中点应为两图均值");
        }
        Ok(())
    }

    /// slide 中点:旧图左移半宽——左半是旧图(右半部分内容),右半是推入的新图。
    #[test]
    fn compose_slide_midpoint_splits_frame() -> color_eyre::Result<()> {
        let out = compose_transition(
            &solid(200, 0, 0),
            &solid(0, 0, 200),
            /*px_w*/ 8,
            /*px_h*/ 4,
            BlendStyle::Slide,
            /*progress_permille*/ 500,
            /*zoom_scale_permille*/ 1120,
        );
        assert_eq!(
            out.get_pixel_checked(0, 0).copied(),
            Some(PxRgb([200, 0, 0])),
            "左缘应仍是旧图"
        );
        assert_eq!(
            out.get_pixel_checked(7, 0).copied(),
            Some(PxRgb([0, 0, 200])),
            "右缘应是推入的新图"
        );
        Ok(())
    }

    /// zoom 端点:进度 0 恰为旧图、进度 1000 恰为新图(缩放与透明度都归位,落定零漂移)。
    #[test]
    fn compose_zoom_endpoints_are_exact() -> color_eyre::Result<()> {
        let endpoints = [(0_u16, PxRgb([200, 0, 0])), (1000_u16, PxRgb([0, 0, 200]))];
        for (progress, expected) in endpoints {
            let out = compose_transition(
                &solid(200, 0, 0),
                &solid(0, 0, 200),
                /*px_w*/ 8,
                /*px_h*/ 8,
                BlendStyle::Zoom,
                progress,
                /*zoom_scale_permille*/ 1120,
            );
            for p in out.pixels() {
                assert_eq!(*p, expected, "进度 {progress}‰ 应为端点原图");
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

        render_halfblock_to(&mut buf, area, &image);

        let top = buf.cell((0, 0)).ok_or_else(|| eyre!("顶 cell 越界"))?;
        assert_eq!(top.fg, Color::Rgb(220, 0, 0), "顶 cell 上半 = 红");
        let bottom = buf.cell((0, 3)).ok_or_else(|| eyre!("底 cell 越界"))?;
        assert_eq!(bottom.bg, Color::Rgb(0, 0, 220), "底 cell 下半 = 蓝");
        Ok(())
    }
}

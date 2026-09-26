//! 主帧渲染入口。

use super::page_morph;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders};

use mineral_config::SearchFocusTransition;

use crate::app::App;
use crate::components::layout::browse::{lyrics, now_playing, sidebar, spectrum};
use crate::components::layout::search::{detail, panel};
use crate::components::layout::shared::compute::{
    Areas, compute, compute_fullscreen, compute_search,
};
use crate::components::layout::shared::marquee::MarqueeCtx;
use crate::components::layout::shared::waveform::WaveformCtx;
use crate::components::layout::shared::{top_status, transform, transport, vinyl};
use crate::image::{BlendStyle, ImageContent, ImageRenderPhase};
use crate::render::ambient;
use crate::runtime::state::SearchFocus;

/// 渲染当前页面；形变期间合成两端稳定排版，封面和播放信息独立移动。
/// 通知与浮层叠在页面之上，最后绘制启动或退出的整屏边框。
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let theme = &app.theme;
    // 回写本帧面积:按键路径(弹菜单求锚点)据此重算布局,不依赖 TTY 查询。
    app.state.frame_area.set(frame.area());
    let layout_cfg = app.state.cfg.tui().layout();
    let normal = compute(frame.area(), layout_cfg);

    // 整屏背景底(在任何布局面板之下):先铺 `theme.background`(普通页也有底色,消除进退
    // 全屏时与沉浸背景的色跳变),再叠氛围场——氛围由滞后跟随门控,进 / 退全屏时背景色慢
    // 半拍淡入 / 淡出,退出时越过恢复的列表界面慢褪。
    paint_backdrop(frame, app, &normal, layout_cfg);

    // 互斥保证 fullscreen / search 两个 Toggle 同时只一个离开 at_min,故顺序判即可。
    if !app.state.browse.fullscreen.at_min() {
        let full = compute_fullscreen(frame.area(), layout_cfg);
        if app.state.browse.fullscreen.at_max() {
            paint_fullscreen(frame, &full, app);
        } else {
            page_morph::fullscreen(frame, &normal, &full, app);
        }
    } else if !app.state.channel_search.active.at_min() {
        let search = compute_search(frame.area(), layout_cfg);
        if app.state.channel_search.active.at_max() {
            paint_search(frame, &search, app, /*cover_in_flight*/ false);
        } else {
            page_morph::search(frame, &normal, &search, app);
        }
    } else {
        paint_browse(frame, &normal, app);
    }

    // topbar 通知层 / 浮层栈:整屏转场(启动扩大 / 退出收缩)期间不画;全屏形变不抑制。
    // 通知锚点恒用常规顶栏行(全屏顶栏已收掉,仍从屏顶向下堆叠)。沉浸进度直接喂
    // 形变缓动值:z 切换期间通知锚点随布局连续插值(居中 ↔ 右上),不瞬移。
    if app.transition.is_none() {
        app.notifications.render(
            frame,
            normal.top_status,
            theme,
            app.state.browse.fullscreen.eased_in_out(),
            &app.notice_hint,
        );
        app.overlays.render(frame, frame.area(), &app.state, theme);
    }

    if let Some(anim) = &app.transition {
        transform::clip_scaled(frame, frame.area(), anim.eased(), app.launch_anchor, theme);
    }
}

/// 常规(浏览态)布局:把各 area 分发给对应组件渲染。
fn paint_browse(frame: &mut Frame<'_>, areas: &Areas, app: &App) {
    let theme = &app.theme;
    top_status::draw(frame, areas.top_status, &app.state, theme);
    sidebar::draw(frame, areas.left, &app.state, theme);
    if let Some(right) = areas.right {
        now_playing::draw(
            frame, right, &app.state, theme, /*cover_in_flight*/ false,
        );
    }
    if let Some(lyr) = areas.lyrics {
        lyrics::draw(frame, lyr, &app.state, theme, lyrics::LyricMode::Compact);
    }
    if let Some(spec) = areas.spectrum {
        spectrum::draw(frame, spec, &app.state.spectrum, theme);
    }
    transport::draw(
        frame,
        areas.transport,
        &app.state.playback,
        &app.state.transport,
        &MarqueeCtx::new(&app.state, theme, /*fade_to*/ theme.base),
        &WaveformCtx::new(&app.state, theme),
        theme,
    );
}

/// Search 稳态布局：prompt 接管顶行，主体为结果与详情面板，播放栏全宽贴底。
///
/// 焦点高亮边框两种过渡(config `search_focus_transition`):`Instant` 时各面板按当前焦点直接
/// 高亮;`Slide` 滑动期把所有面板边框压暗,改由一个 accent 浮动环从旧面板矩形 lerp 到新面板。
///
/// `cover_in_flight`:page morph 封面飞行层已接管主图(now_playing 封面 / detail 头图),
/// 面板跳过自画防双画。
fn paint_search(frame: &mut Frame<'_>, areas: &Areas, app: &App, cover_in_flight: bool) {
    let theme = &app.theme;
    let rs = &app.state.channel_search;
    let sliding = matches!(
        app.state.cfg.tui().animation().search_focus_transition(),
        SearchFocusTransition::Slide
    ) && !rs.focus_ring.settled();
    // 滑动期所有面板边框压暗,高亮交给浮动环;否则当前焦点面板边框高亮。
    let border_focused = |panel: SearchFocus| !sliding && rs.focus == panel;

    if let Some(prompt) = areas.search_prompt {
        panel::draw_prompt(
            frame,
            prompt,
            rs,
            theme,
            app.state.cfg.sources(),
            border_focused(SearchFocus::Prompt),
        );
    }
    if let Some(left) = nonempty(areas.left) {
        panel::draw_results(
            frame,
            left,
            &app.state,
            theme,
            border_focused(SearchFocus::Results),
        );
    }
    if let Some(right) = areas.right.and_then(nonempty) {
        detail::draw(
            frame,
            right,
            &app.state,
            theme,
            border_focused(SearchFocus::Detail),
            cover_in_flight,
        );
    }
    // 焦点环:滑动期画 accent 浮动边框,从旧面板矩形几何插值到新面板矩形(border-only,不清内容)。
    if sliding
        && let (Some(from), Some(to)) = (
            search_focus_rect(areas, rs.prev_focus),
            search_focus_rect(areas, rs.focus),
        )
    {
        let ring = transform::lerp_rect(from, to, rs.focus_ring.eased_in_out());
        frame.render_widget(
            Block::new()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(theme.accent)),
            ring,
        );
    }
    // chip 下拉(source/kind)画在最后,盖在 results 面板之上。
    if let Some(prompt) = areas.search_prompt {
        panel::draw_prompt_dropdown(frame, prompt, &app.state, theme);
    }
    transport::draw(
        frame,
        areas.transport,
        &app.state.playback,
        &app.state.transport,
        &MarqueeCtx::new(&app.state, theme, /*fade_to*/ theme.base),
        &WaveformCtx::new(&app.state, theme),
        theme,
    );
}

/// 焦点对应的面板矩形(prompt 行 / results 左 / detail 右);该面板在当前端点不存在为 `None`。
/// 焦点环滑动据此取两端矩形插值。
fn search_focus_rect(areas: &Areas, focus: SearchFocus) -> Option<Rect> {
    match focus {
        SearchFocus::Prompt => areas.search_prompt,
        SearchFocus::Results => nonempty(areas.left),
        SearchFocus::Detail => areas.right.and_then(nonempty),
    }
}

/// 全屏稳态：频谱、播放栏、封面与沉浸歌词。
fn paint_fullscreen(frame: &mut Frame<'_>, areas: &Areas, app: &App) {
    let theme = &app.theme;
    if let Some(spec) = areas.spectrum.and_then(nonempty) {
        spectrum::draw(frame, spec, &app.state.spectrum, theme);
    }
    // marquee 边缘 fade 的目标 = transport 面实际背景(ambient 场色);采到 Reset
    // (ambient 关 / ANSI 主题)回落 base,与非全屏调用点一致。
    let marquee_fade_to =
        match crate::components::layout::shared::text::center_bg(frame, areas.transport) {
            bg @ ratatui::style::Color::Rgb(..) => bg,
            _ => theme.base,
        };
    transport::draw(
        frame,
        areas.transport,
        &app.state.playback,
        &app.state.transport,
        &MarqueeCtx::new(&app.state, theme, marquee_fade_to),
        &WaveformCtx::new(&app.state, theme),
        theme,
    );
    if let Some(c) = areas.cover.and_then(nonempty) {
        draw_fullscreen_cover(frame, c, areas.cover, app);
    }
    if let Some(lyr) = areas.lyrics.and_then(nonempty) {
        lyrics::draw(frame, lyr, &app.state, theme, lyrics::LyricMode::Immersive);
    }
}

/// 整屏背景底:布局面板之下的两层——先 `theme.background` 纯色填充,再叠氛围渐变场。
///
/// 普通页面本无底色(逐格终端默认),进 / 退全屏时整屏刷成沉浸底会瞬跳;这里给普通页也
/// 铺 `theme.background`(默认 = `base` = 氛围场在浓度 0 时的底色),两端连续。氛围场再叠
/// 其上,浓度由**滞后跟随**进度驱动(慢半拍跟随全屏形变)。Sixel / iTerm2 实际图区
/// 避开背景重绘，防止首 cell 改色触发图协议载荷重发；其他区域保留动态背景。
fn paint_backdrop(
    frame: &mut Frame<'_>,
    app: &App,
    normal: &Areas,
    layout_cfg: &mineral_config::LayoutConfig,
) {
    let area = frame.area();
    let skip = backdrop_skip(app, area, normal, layout_cfg);
    fill_bg(frame.buffer_mut(), area, app.theme.background, skip);
    draw_ambient(frame, app, skip);
}

/// 本帧铺底 / 氛围都要挖的真图洞(防每帧改图 cell 的 bg 触发图协议载荷 diff 重发):
///   - 全屏稳态(`at_max`):全屏封面真图区(见 [`ambient_skip_rect`]);
///   - 退出残留期(几何已回列表 `at_min`、氛围仍在褪):now_playing 面板的真图封面区;
///   - 其余(Kitty / halfblock / 形变途中):无洞。
fn backdrop_skip(
    app: &App,
    area: Rect,
    normal: &Areas,
    layout_cfg: &mineral_config::LayoutConfig,
) -> Option<Rect> {
    let fullscreen = &app.state.browse.fullscreen;
    if fullscreen.at_max() {
        return ambient_skip_rect(app, compute_fullscreen(area, layout_cfg).cover);
    }
    if fullscreen.at_min() && app.state.browse.ambient_reveal.active() {
        return now_playing_cover_skip(app, normal.right);
    }
    None
}

/// now_playing 面板当前 place 的真图封面视觉区(用于退出残留期挖洞);面板不画真图
/// (无选中 / 无图 / 协议未就绪 / halfblock 兜底)时为 `None`。
fn now_playing_cover_skip(app: &App, right: Option<Rect>) -> Option<Rect> {
    let right = nonempty(right?)?;
    let url = now_playing::main_cover::url(&app.state)?;
    let [cover_sec, _, _] = now_playing::main_cover::sections(right)?;
    app.state.images.ready_area(&url, nonempty(cover_sec)?)
}

/// 整屏背景填充:把 `area` 内每格底色刷成 `color`;`color == Reset` 时不改已有背景。
/// `skip` 区不刷(见 [`ambient_skip_rect`])。
fn fill_bg(buf: &mut Buffer, area: Rect, color: Color, skip: Option<Rect>) {
    if matches!(color, Color::Reset) {
        return;
    }
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if skip.is_some_and(|hole| hole.contains(Position::new(x, y))) {
                continue;
            }
            if let Some(cell) = buf.cell_mut((x, y)) {
                cell.set_bg(color);
            }
        }
    }
}

/// 氛围渐变场:整屏铺当前封面调色板驱动的渐变场,浓度由**滞后跟随**进度驱动——进 / 退全屏
/// 时背景色慢半拍淡入 / 淡出,退出时几何已回列表、场仍越过列表慢褪(故门控用
/// `ambient_reveal.active()` 而非全屏几何)。跟随静止在关态时整段跳过(背景填充已铺底);
/// 功能关且色板淡出已到底、或 ANSI 主题无真彩底色时同样跳过。
fn draw_ambient(frame: &mut Frame<'_>, app: &App, skip: Option<Rect>) {
    if !app.state.browse.ambient_reveal.active() {
        return;
    }
    let cfg = app.state.cfg.tui().ambient();
    if !*cfg.enabled() && app.ambient.settled_at_base() {
        return;
    }
    let Some(base) = ambient::rgb_of(app.theme.base) else {
        return;
    };
    let area = frame.area();
    ambient::render(
        frame.buffer_mut(),
        area,
        &app.ambient,
        base,
        cfg,
        app.state.browse.ambient_reveal.progress(),
        app.ambient_pulse.level_permille(cfg.pulse()),
        skip,
    );
}

/// Sixel / iTerm2 的实际图片外框避开背景重绘，防止首 cell 改色导致重发载荷。
/// Kitty 逐格保留背景；halfblock 和转场按透明度合成，这些路径都不挖洞。
fn ambient_skip_rect(app: &App, cover: Option<Rect>) -> Option<Rect> {
    if !app.state.browse.fullscreen.at_max() || app.state.images.transition.is_some() {
        return None;
    }
    let track = app.state.playback.track.as_ref()?;
    let url = track.cover_url.as_ref()?;
    app.state.images.ready_area(url, cover.and_then(nonempty)?)
}

/// 全屏独立封面跟随在播曲；形变中只画 halfblock，稳态全屏才使用终端图片成品
/// (避免形变期每帧尺寸变化导致重复编码)。无在播曲时画待机唱片纹(纯 cell、逐帧
/// 重画安全,形变 / 稳态同一条路),盘面下段叠 `nothing playing` 提示。
///
/// # Params:
///   - `steady_cover`: 终态全屏封面区,进入方向的形变期按它预热当前曲协议编码
pub(super) fn draw_fullscreen_cover(
    frame: &mut Frame<'_>,
    area: Rect,
    steady_cover: Option<Rect>,
    app: &App,
) {
    let theme = &app.theme;
    let Some(track) = app.state.playback.track.as_ref() else {
        vinyl::render(frame, area, &app.state.vinyl, theme);
        return;
    };
    if app.state.browse.fullscreen.at_max() {
        // 切歌转场窗口:新旧两图像素级合成 halfblock(纯 cell,逐帧重画安全),恰好盖住
        // 新图的离线编码期;同时按当前尺寸预热新图协议((url, dims) 去重),推满落定
        // 直接 place 高清零闪。缺任一图回落常规路径。
        if let Some(transition) = app.state.images.transition.as_ref() {
            let style = BlendStyle::from(*app.state.cfg.tui().cover_transition().style());
            app.state.images.render(
                ImageContent::Blend {
                    from: &transition.from_url,
                    to: &transition.to_url,
                    progress: transition.anim.eased_in_out(),
                    style,
                    advance: transition.advance,
                },
                area,
                frame.buffer_mut(),
                ImageRenderPhase::Stable,
            );
            app.state.images.prepare(&transition.to_url, area);
            prewarm_upcoming(app, area);
            return;
        }
        app.state.images.render(
            ImageContent::Display {
                url: track.cover_url.as_ref(),
            },
            area,
            frame.buffer_mut(),
            ImageRenderPhase::Stable,
        );
        // 全屏稳态封面区尺寸固定:顺手把后续若干首按同尺寸提前编码,自动切歌时协议已就绪、
        // 直接 place，避免切歌瞬间出现空白。
        prewarm_upcoming(app, area);
    } else {
        // 形变期：halfblock 随封面区长大；无真实图片时保留背景。
        app.state.images.render(
            ImageContent::Display {
                url: track.cover_url.as_ref(),
            },
            area,
            frame.buffer_mut(),
            ImageRenderPhase::Resizing,
        );
        // 进入方向:终态封面区固定可知,按它把当前曲的协议编码与形变动画并行预热,
        // 落定即命中直接上真图,消「落定后先糊后清晰」的等待。`(url, dims)` 去重,整段
        // 形变只投一次;**绝不按形变中逐帧漂移的 `area` 预热**(那是 churn)。退出方向
        // 不预热——面板尺寸协议在多尺寸槽位下仍在缓存,回去即命中。
        if app.state.browse.fullscreen.on()
            && let (Some(url), Some(steady)) =
                (track.cover_url.as_ref(), steady_cover.and_then(nonempty))
        {
            app.state.images.prepare(url, steady);
        }
    }
}

/// 全屏稳态:给在播曲前后各 `prefetch.prewarm_ahead` 首(图已就绪者)的封面按当前尺寸提前
/// 编码,切歌(`n` / `p` / 自动接续)时协议已就绪、直接 place 无闪,切歌转场也才拿得到进场图。
/// 邻居按播放模式环回算(环回那一端也要预热);无在播 / 该首无封面 → 跳过。
fn prewarm_upcoming(app: &App, area: Rect) {
    let ahead = *app.state.cfg.tui().prefetch().prewarm_ahead();
    for idx in app.state.queue_neighbor_indexes(ahead) {
        if let Some(url) = app
            .state
            .player
            .queue
            .get(idx)
            .and_then(|s| s.cover_url.as_ref())
        {
            app.state.images.prepare(url, area);
        }
    }
}

/// 非空矩形过滤:宽高都 > 0 才返回 `Some`,供 `.and_then` 链跳过零面积面板。
pub(super) fn nonempty(r: Rect) -> Option<Rect> {
    (r.width > 0 && r.height > 0).then_some(r)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::render::anim::Toggle;
    use crate::test_support::app_with_queue;

    /// 全屏形变只能按两端稳态尺寸预热；中间帧不得按逐帧尺寸追加编码。测试不先渲染常规
    /// 帧，避免常规封面尺寸恰好与端点相同时掩盖预热请求；落定帧必须命中相同
    /// `(url, dims)` 去重键。
    #[test]
    fn fullscreen_morph_prewarms_steady_cover_once() -> color_eyre::Result<()> {
        use std::sync::Arc;
        use std::time::{Duration, Instant};

        use mineral_model::{MediaUrl, PlaylistId, SourceKind};

        use crate::test_support::app_with_library;

        let mut app = app_with_library(3, /*sel_track*/ 0)?;

        let url = MediaUrl::remote("https://x.y/cover.jpg")?;
        let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
        if let Some(sv) = app
            .state
            .library
            .tracks
            .get_mut(&pid)
            .and_then(|views| views.get_mut(0))
        {
            sv.data.song.cover_url = Some(url.clone());
            app.state.playback.track = Some(sv.data.song.clone());
        }
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(64, 64));
        app.state.images.cache.insert_test(&url, Arc::new(img));
        // 关掉滚动防抖早退(置选中变化于防抖窗口之外),让稳态帧真正派发编码。
        app.state.browse.nav.last_sel_change = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);

        let mut t = Terminal::new(TestBackend::new(120, 40))?;

        // 从空 pending 开始形变,使首帧暴露端点预热请求。
        app.state.browse.fullscreen.set(true);
        let mut morph_pending = app.state.images.encode_pending.borrow().clone();
        assert!(morph_pending.is_empty(), "前置:尚未渲染,pending 为空");
        for frame_no in 0..5 {
            app.state.browse.fullscreen.tick();
            assert!(
                !app.state.browse.fullscreen.settled(),
                "测试需停留在形变中途"
            );
            t.draw(|f| super::draw(f, &app))?;
            if frame_no == 0 {
                morph_pending = app.state.images.encode_pending.borrow().clone();
                assert!(
                    !morph_pending.is_empty(),
                    "首个形变帧应派发端点稳态尺寸预热"
                );
            } else {
                assert_eq!(
                    *app.state.images.encode_pending.borrow(),
                    morph_pending,
                    "后续形变帧不应追加封面编码派发(churn)"
                );
            }
        }

        for _ in 0..1_000 {
            if app.state.browse.fullscreen.settled() {
                break;
            }
            app.state.browse.fullscreen.tick();
        }
        assert!(app.state.browse.fullscreen.settled(), "形变应在上限内落定");
        t.draw(|f| super::draw(f, &app))?;
        assert_eq!(
            *app.state.images.encode_pending.borrow(),
            morph_pending,
            "稳态渲染应命中预热的同一 (url, dims) 去重键"
        );
        Ok(())
    }

    /// Page morph 只为两端稳态尺寸派发封面编码；中间帧命中 `(url, dims)` 去重。
    #[test]
    fn search_morph_prewarms_endpoints_without_churn() -> color_eyre::Result<()> {
        use crate::render::anim::Toggle;
        use crate::test_support::app_in_search_morph;

        let mut app = app_in_search_morph(/*cache_browse*/ true, /*cache_detail*/ true)?;
        let mut active = Toggle::new(8);
        active.set(true);
        active.tick();
        app.state.channel_search.active = active;
        let mut t = Terminal::new(TestBackend::new(120, 40))?;
        t.draw(|f| super::draw(f, &app))?;
        let pending = app.state.images.encode_pending.borrow().clone();
        assert!(!pending.is_empty(), "首个形变帧应预热端点封面编码");
        for _ in 0..3 {
            app.state.channel_search.active.tick();
            t.draw(|f| super::draw(f, &app))?;
        }
        assert_eq!(
            *app.state.images.encode_pending.borrow(),
            pending,
            "后续形变帧不应追加编码派发(churn)"
        );
        Ok(())
    }

    /// 全屏稳态：下一首(queue 中在播曲的紧邻后继)应按目标像素尺寸提前准备终端图片，
    /// 自动切歌时可以直接 place。
    #[test]
    fn fullscreen_steady_prewarms_next_cover() -> color_eyre::Result<()> {
        use std::sync::Arc;

        use mineral_model::MediaUrl;

        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        // 给队列每首塞封面 URL;在播曲(queue[0])与下一首(queue[1])的图放进 cache
        // —— 预编码要求图已就绪(否则该首仍等 fetch,后续帧再预热)。
        for i in 0..3 {
            let url = MediaUrl::remote(&format!("https://prewarm/{i}.jpg"))?;
            if let Some(s) = app.state.player.queue.get_mut(i) {
                s.cover_url = Some(url.clone());
            }
            if i <= 1 {
                let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(64, 64));
                app.state.images.cache.insert_test(&url, Arc::new(img));
            }
        }
        // 重新同步在播曲(带上刚塞的封面 URL)。
        app.state.playback.track = app.state.player.queue.first().cloned();
        // 稳态全屏:一步推到满值。
        let mut fs = Toggle::new(1);
        fs.set(true);
        fs.tick();
        app.state.browse.fullscreen = fs;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;

        let next_url = MediaUrl::remote("https://prewarm/1.jpg")?;
        let warmed = app
            .state
            .images
            .encode_pending
            .borrow()
            .iter()
            .any(|key| key.matches_url(&next_url));
        assert!(warmed, "全屏稳态应提前编码下一首封面");
        Ok(())
    }

    /// 全屏稳态也预热上一首封面:`p` 落回去那首同样要进场图已解码,转场才开得起来。
    #[test]
    fn fullscreen_steady_prewarms_previous_cover() -> color_eyre::Result<()> {
        use std::sync::Arc;

        use mineral_model::MediaUrl;

        // 在播曲取中间那首,前后各有邻居。
        let mut app = app_with_queue(3, /*current_idx*/ 1)?;
        for i in 0..3 {
            let url = MediaUrl::remote(&format!("https://prewarm/{i}.jpg"))?;
            if let Some(s) = app.state.player.queue.get_mut(i) {
                s.cover_url = Some(url.clone());
            }
            let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(64, 64));
            app.state.images.cache.insert_test(&url, Arc::new(img));
        }
        app.state.playback.track = app.state.player.queue.get(1).cloned();
        let mut fs = Toggle::new(1);
        fs.set(true);
        fs.tick();
        app.state.browse.fullscreen = fs;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;

        let pending = app.state.images.encode_pending.borrow();
        for (idx, label) in [(0, "上一首"), (2, "下一首")] {
            let url = MediaUrl::remote(&format!("https://prewarm/{idx}.jpg"))?;
            assert!(
                pending.iter().any(|key| key.matches_url(&url)),
                "全屏稳态应提前编码{label}封面"
            );
        }
        Ok(())
    }
}

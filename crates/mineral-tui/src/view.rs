//! 主帧渲染入口。

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
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
use crate::components::layout::shared::{cover_image, top_status, transform, transport, vinyl};
use crate::render::ambient;
use crate::runtime::state::SearchFocus;

/// 渲染一帧:全屏态 / 形变走全屏 paint(几何由 `compute_fullscreen` 与 `morph_areas` 给出);
/// search 布局态走 search paint(端点几何 `compute_search`、形变中途 `morph_search`);否则
/// 浏览态常规 paint。通知 / 浮层叠在最上(整屏转场期间不画);最后叠启动 / 退出缩放边框。
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let theme = &app.theme;
    // 回写本帧面积:按键路径(弹菜单求锚点)据此重算布局,不依赖 TTY 查询。
    app.state.frame_area.set(frame.area());
    let layout_cfg = app.state.cfg.tui().layout();
    let normal = compute(frame.area(), layout_cfg);

    // 互斥保证 fullscreen / search 两个 Toggle 同时只一个离开 at_min,故顺序判即可。
    if !app.state.browse.fullscreen.at_min() {
        let full = compute_fullscreen(frame.area(), layout_cfg);
        // 终态全屏封面区在形变全程固定可知,形变分支据此预热当前曲的协议编码。
        let steady_cover = full.cover;
        let areas = if app.state.browse.fullscreen.at_max() {
            full
        } else {
            transform::morph_areas(&normal, &full, app.state.browse.fullscreen.eased_in_out())
        };
        paint_fullscreen(frame, &areas, steady_cover, app);
    } else if !app.state.channel_search.active.at_min() {
        let search = compute_search(frame.area(), layout_cfg);
        let areas = if app.state.channel_search.active.at_max() {
            search
        } else {
            transform::morph_search(
                &normal,
                &search,
                app.state.channel_search.active.eased_in_out(),
            )
        };
        paint_search(frame, &areas, app);
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
        now_playing::draw(frame, right, &app.state, &app.picker, theme);
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
        &MarqueeCtx::new(&app.state, theme, /*fade_to*/ theme.base),
        &WaveformCtx::new(&app.state, theme),
        theme,
    );
}

/// Search 布局:prompt 框接管顶行(稳态无 status bar;morph 收缩中途 browse 顶栏短暂可见),其下
/// results(左)/ detail(右)面板,transport 全宽贴底。
/// 左/右内容随形变方向画「出发端」(进场画浏览、退场画 search),到达端点才换,进退场对称、
/// 不在起点瞬切;lyrics / spectrum 是浏览专属面板,随 rect 收缩/长出退进场。
///
/// 焦点高亮边框两种过渡(config `search_focus_transition`):`Instant` 时各面板按当前焦点直接
/// 高亮;`Slide` 滑动期把所有面板边框压暗,改由一个 accent 浮动环从旧面板矩形 lerp 到新面板。
fn paint_search(frame: &mut Frame<'_>, areas: &Areas, app: &App) {
    let theme = &app.theme;
    let rs = &app.state.channel_search;
    let sliding = matches!(
        app.state.cfg.tui().animation().search_focus_transition(),
        SearchFocusTransition::Slide
    ) && !rs.focus_ring.settled();
    // 滑动期所有面板边框压暗,高亮交给浮动环;否则当前焦点面板边框高亮。
    let border_focused = |panel: SearchFocus| !sliding && rs.focus == panel;

    // 左/右面板内容始终画「出发端」,到达端点才换——形变进退场对称、不在起点瞬切:
    //   进场途中(朝 search 去) → 画浏览内容(playlists / now_playing)随框飞向 search 位,到
    //     at_max 换 results / detail;
    //   退场途中(朝 browse 去) → 仍画 results / detail 随框缩回 browse 位,到 at_min 由浏览态
    //     paint 接管(esc 瞬间不把 results 跳成 playlists,消除跳变 + 起步卡顿)。
    let show_browse = rs.active.on() && !rs.active.at_max();
    // morph 收缩中途的 browse 顶栏(非零面积)才画;稳态 search 顶栏零面积,跳过。
    if let Some(top) = nonempty(areas.top_status) {
        top_status::draw(frame, top, &app.state, theme);
    }
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
        if show_browse {
            sidebar::draw(frame, left, &app.state, theme);
        } else {
            panel::draw_results(
                frame,
                left,
                &app.state,
                theme,
                border_focused(SearchFocus::Results),
            );
        }
    }
    if let Some(right) = areas.right.and_then(nonempty) {
        if show_browse {
            now_playing::draw(frame, right, &app.state, &app.picker, theme);
        } else {
            detail::draw(
                frame,
                right,
                &app.state,
                &app.picker,
                theme,
                border_focused(SearchFocus::Detail),
            );
        }
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
    // 形变期收缩中的 lyrics / spectrum 仍画浏览内容(稳态 search 端点为 None,自动跳过)。
    if let Some(lyr) = areas.lyrics.and_then(nonempty) {
        lyrics::draw(frame, lyr, &app.state, theme, lyrics::LyricMode::Compact);
    }
    if let Some(spec) = areas.spectrum.and_then(nonempty) {
        spectrum::draw(frame, spec, &app.state.spectrum, theme);
    }
    // chip 下拉(source/kind)画在最后,盖在 results 面板之上。
    if let Some(prompt) = areas.search_prompt {
        panel::draw_prompt_dropdown(frame, prompt, &app.state, theme);
    }
    transport::draw(
        frame,
        areas.transport,
        &app.state.playback,
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

/// 全屏 / 形变布局:先整屏铺氛围背景(只写 bg,后画组件的 fg-only 样式天然透出)
/// → 消失面板渲进收缩 rect(小到自动空白)→ spectrum → transport → cover
/// → lyrics(歌词最后画,对穿交错时压在封面上保持可读)。
///
/// # Params:
///   - `steady_cover`: 终态全屏封面区(形变全程固定),形变分支按它预热协议编码
fn paint_fullscreen(frame: &mut Frame<'_>, areas: &Areas, steady_cover: Option<Rect>, app: &App) {
    draw_ambient(frame, app);
    let theme = &app.theme;
    if let Some(r) = nonempty(areas.top_status) {
        top_status::draw(frame, r, &app.state, theme);
    }
    if let Some(r) = nonempty(areas.left) {
        sidebar::draw(frame, r, &app.state, theme);
    }
    if let Some(r) = areas.right.and_then(nonempty) {
        now_playing::draw(frame, r, &app.state, &app.picker, theme);
    }
    if let Some(spec) = areas.spectrum.and_then(nonempty) {
        spectrum::draw(frame, spec, &app.state.spectrum, theme);
    }
    transport::draw(
        frame,
        areas.transport,
        &app.state.playback,
        &MarqueeCtx::new(&app.state, theme, /*fade_to*/ theme.base),
        &WaveformCtx::new(&app.state, theme),
        theme,
    );
    if let Some(c) = areas.cover.and_then(nonempty) {
        draw_fullscreen_cover(frame, c, steady_cover, app);
    }
    if let Some(lyr) = areas.lyrics.and_then(nonempty) {
        lyrics::draw(frame, lyr, &app.state, theme, lyrics::LyricMode::Immersive);
    }
}

/// 氛围背景:整屏铺当前封面调色板驱动的渐变场(功能开着、或关闭后仍在淡出途中才画;
/// 浓度乘全屏形变进度,进场随形变淡入、退场随形变收干净)。ANSI 主题无真彩底色,跳过。
fn draw_ambient(frame: &mut Frame<'_>, app: &App) {
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
        app.state.browse.fullscreen.eased_in_out(),
    );
}

/// 全屏独立封面:跟**在播曲**;形变中画 halfblock / 程序化色块(便宜),稳态全屏才上真图
/// (避免形变期每帧尺寸变导致 protocol 重编码)。无在播曲时画待机唱片纹(纯 cell、逐帧
/// 重画安全,形变 / 稳态同一条路),盘面下段叠 `nothing playing` 提示。
///
/// # Params:
///   - `steady_cover`: 终态全屏封面区,进入方向的形变期按它预热当前曲协议编码
fn draw_fullscreen_cover(frame: &mut Frame<'_>, area: Rect, steady_cover: Option<Rect>, app: &App) {
    let theme = &app.theme;
    let Some(track) = app.state.playback.track.as_ref() else {
        vinyl::render(frame, area, &app.state.vinyl, theme);
        return;
    };
    let seed = track
        .album
        .as_ref()
        .map_or_else(|| track.name.clone(), |a| a.name.clone());
    if app.state.browse.fullscreen.at_max() {
        // 切歌转场窗口:新旧两图像素级合成 halfblock(纯 cell,逐帧重画安全),恰好盖住
        // 新图的离线编码期;同时按当前尺寸预热新图协议((url, dims) 去重),推满落定
        // 直接 place 高清零闪。缺任一图回落常规路径。
        if let Some(transition) = app.state.covers.transition.as_ref()
            && cover_image::render_transition(frame, area, transition, &app.state, &app.picker)
        {
            cover_image::prewarm(&app.state, &app.picker, area, &transition.to_url);
            prewarm_upcoming(app, area);
            return;
        }
        cover_image::render_or_fallback(
            frame,
            area,
            track.cover_url.as_ref(),
            &app.state,
            &app.picker,
            theme,
            &seed,
        );
        // 全屏稳态封面区尺寸固定:顺手把后续若干首按同尺寸提前编码,自动切歌时协议已就绪、
        // 直接 place,消掉切歌瞬间的程序化占位闪。
        prewarm_upcoming(app, area);
    } else {
        // 形变期:halfblock 真图(命中缓存)随封面区长大,无图退程序化色块;均不碰 kitty 协议。
        cover_image::render_morph(
            frame,
            area,
            track.cover_url.as_ref(),
            &app.state,
            &app.picker,
            theme,
            &seed,
        );
        // 进入方向:终态封面区固定可知,按它把当前曲的协议编码与形变动画并行预热,
        // 落定即命中直接上真图,消「落定后先糊后清晰」的等待。`(url, dims)` 去重,整段
        // 形变只投一次;**绝不按形变中逐帧漂移的 `area` 预热**(那是 churn)。退出方向
        // 不预热——面板尺寸协议在多尺寸槽位下仍在缓存,回去即命中。
        if app.state.browse.fullscreen.on()
            && let (Some(url), Some(steady)) =
                (track.cover_url.as_ref(), steady_cover.and_then(nonempty))
        {
            cover_image::prewarm(&app.state, &app.picker, steady, url);
        }
    }
}

/// 全屏稳态:给在播曲之后 `prefetch.prewarm_ahead` 首(图已就绪者)的封面按当前尺寸提前编码,
/// 自动切歌时协议已就绪、直接 place 无闪。无在播 / 队尾越界 / 该首无封面 → 跳过。
fn prewarm_upcoming(app: &App, area: Rect) {
    let Some(pos) = app.state.queue_current_index() else {
        return;
    };
    for d in 1..=*app.state.cfg.tui().prefetch().prewarm_ahead() {
        if let Some(url) = app
            .state
            .player
            .queue
            .get(pos.saturating_add(d))
            .and_then(|s| s.cover_url.as_ref())
        {
            cover_image::prewarm(&app.state, &app.picker, area, url);
        }
    }
}

/// 非空矩形过滤:宽高都 > 0 才返回 `Some`,供 `.and_then` 链跳过零面积面板。
fn nonempty(r: Rect) -> Option<Rect> {
    (r.width > 0 && r.height > 0).then_some(r)
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::style::Color;

    use crate::render::anim::{Toggle, Transition};
    use crate::test_support::{app_in_fullscreen, app_with_queue, app_with_search};

    /// 全屏进入形变:不按逐帧漂移的 dims 派发编码(churn),但按**终态全屏尺寸**预热
    /// 恰好一次——编码与形变动画并行,落定即命中。从零 pending 起步(不先渲常规帧:
    /// 常规面板与全屏封面的正方 cell 尺寸在部分终端几何下会撞同值,基线派发会掩蔽预热
    /// 那一条)。三段断言:
    /// ① 首个形变帧后 pending 恰好一条(预热);
    /// ② 后续形变帧 pending 不再变(逐帧漂移 dims 一条都没混进来);
    /// ③ 落定 at_max 渲稳态帧后 pending 仍不变——稳态渲染与预热撞同一 `(url, dims)` 去重,
    ///    反证预热用的正是终态尺寸(错一个 cell 都会多出一条)。
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
            sv.data.cover_url = Some(url.clone());
            app.state.playback.track = Some(sv.data.clone());
        }
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(64, 64));
        app.state.covers.cache.insert(&url, Arc::new(img));
        // 关掉滚动防抖早退(置选中变化于防抖窗口之外),让稳态帧真正派发编码。
        app.state.browse.nav.last_sel_change = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);

        let mut t = Terminal::new(TestBackend::new(120, 40))?;

        // 进入全屏,推进若干形变帧(均 `!settled`)。首帧即应派发终态尺寸预热恰好一条;
        // 之后每帧 pending 定格——证明没按逐帧漂移 dims 追加派发。
        app.state.browse.fullscreen.set(true);
        let mut morph_pending = app.state.covers.encode_pending.borrow().clone();
        assert!(morph_pending.is_empty(), "前置:尚未渲染,pending 为空");
        for frame_no in 0..5 {
            app.state.browse.fullscreen.tick();
            assert!(
                !app.state.browse.fullscreen.settled(),
                "测试需停留在形变中途"
            );
            t.draw(|f| super::draw(f, &app))?;
            if frame_no == 0 {
                morph_pending = app.state.covers.encode_pending.borrow().clone();
                assert_eq!(
                    morph_pending.len(),
                    1,
                    "首个形变帧应按终态全屏尺寸预热恰好一条"
                );
            } else {
                assert_eq!(
                    *app.state.covers.encode_pending.borrow(),
                    morph_pending,
                    "后续形变帧不应追加封面编码派发(churn)"
                );
            }
        }

        // 推进到落定(at_max),渲稳态全屏帧:稳态渲染的派发应与预热同 key 去重,
        // pending 纹丝不动 —— 反证预热 dims 与稳态渲染 dims 完全一致。
        for _ in 0..1_000 {
            if app.state.browse.fullscreen.settled() {
                break;
            }
            app.state.browse.fullscreen.tick();
        }
        assert!(app.state.browse.fullscreen.settled(), "形变应在上限内落定");
        t.draw(|f| super::draw(f, &app))?;
        assert_eq!(
            *app.state.covers.encode_pending.borrow(),
            morph_pending,
            "稳态渲染应命中预热的同 (url, dims) 去重,不再追加派发"
        );
        Ok(())
    }

    /// 全屏形变中途:在播曲封面已在 `covers.cache` → 封面区渲 halfblock 真图(像素色),
    /// 而非程序化主题色块。用纯品红测试图(UI 别处不会出现 `Rgb(255,0,255)`),扫全 buffer
    /// 断言存在该 fg 的 cell —— 不依赖 morph 中途封面区精确坐标。
    #[test]
    fn fullscreen_morph_paints_real_cover_as_halfblock() -> color_eyre::Result<()> {
        use std::sync::Arc;

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
            sv.data.cover_url = Some(url.clone());
            app.state.playback.track = Some(sv.data.clone());
        }
        let mut img = image::RgbImage::new(64, 64);
        for p in img.pixels_mut() {
            *p = image::Rgb([255, 0, 255]);
        }
        app.state
            .covers
            .cache
            .insert(&url, Arc::new(image::DynamicImage::ImageRgb8(img)));

        app.state.browse.fullscreen.set(true);
        for _ in 0..5 {
            app.state.browse.fullscreen.tick();
        }
        assert!(!app.state.browse.fullscreen.settled(), "测试需停留形变中途");

        let mut t = Terminal::new(TestBackend::new(120, 40))?;
        t.draw(|f| super::draw(f, &app))?;

        let magenta = Color::Rgb(255, 0, 255);
        let buf = t.backend().buffer();
        let mut count = 0usize;
        for y in 0..40u16 {
            for x in 0..120u16 {
                if buf.cell((x, y)).is_some_and(|c| c.fg == magenta) {
                    count = count.saturating_add(1);
                }
            }
        }
        assert!(
            count > 0,
            "形变期封面区应有 halfblock 真图(品红)cell,实得 {count}"
        );
        Ok(())
    }

    /// 全屏形变中途:有 `cover_url` 但图未入 `covers.cache` → 退程序化色块,封面区不应出现
    /// 真图像素色(品红)。与上一例对照,证明只在缓存命中时上真图。
    #[test]
    fn fullscreen_morph_without_cached_image_stays_procedural() -> color_eyre::Result<()> {
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
            sv.data.cover_url = Some(url.clone());
            app.state.playback.track = Some(sv.data.clone());
        }
        // 故意不往 covers.cache 塞图。

        app.state.browse.fullscreen.set(true);
        for _ in 0..5 {
            app.state.browse.fullscreen.tick();
        }
        assert!(!app.state.browse.fullscreen.settled(), "测试需停留形变中途");

        let mut t = Terminal::new(TestBackend::new(120, 40))?;
        t.draw(|f| super::draw(f, &app))?;

        let magenta = Color::Rgb(255, 0, 255);
        let buf = t.backend().buffer();
        for y in 0..40u16 {
            for x in 0..120u16 {
                assert!(
                    !buf.cell((x, y)).is_some_and(|c| c.fg == magenta),
                    "无缓存图时不应出现真图像素色"
                );
            }
        }
        Ok(())
    }

    /// 回归:切到全屏**落定瞬间**的 hash 闪。稳态首帧该曲全屏尺寸 kitty 协议尚未编码(在途),
    /// 封面区应继续画 halfblock 真图、不得退程序化 hash 色块。构造:稳态全屏 + 缓存图就绪 +
    /// `covers.protocols` 空(逼走「编码在途」回退路径)+ 脱离滚动防抖。断言全 buffer 仍含
    /// 真图像素色(品红)。
    #[test]
    fn fullscreen_steady_pending_encode_shows_halfblock_not_hash() -> color_eyre::Result<()> {
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
            sv.data.cover_url = Some(url.clone());
            app.state.playback.track = Some(sv.data.clone());
        }
        let mut img = image::RgbImage::new(64, 64);
        for p in img.pixels_mut() {
            *p = image::Rgb([255, 0, 255]);
        }
        app.state
            .covers
            .cache
            .insert(&url, Arc::new(image::DynamicImage::ImageRgb8(img)));
        // 脱离滚动防抖,确保不是 `is_scrolling` 早退(那会留空、非 hash,测错原因)。
        app.state.browse.nav.last_sel_change = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);

        // 全屏稳态(一步到位);不放任何已编码协议 → 首帧走「kitty 编码在途」回退。
        let mut fs = Toggle::new(1);
        fs.set(true);
        fs.tick();
        app.state.browse.fullscreen = fs;

        let mut t = Terminal::new(TestBackend::new(120, 40))?;
        t.draw(|f| super::draw(f, &app))?;

        let magenta = Color::Rgb(255, 0, 255);
        let buf = t.backend().buffer();
        let mut count = 0usize;
        for y in 0..40u16 {
            for x in 0..120u16 {
                if buf.cell((x, y)).is_some_and(|c| c.fg == magenta) {
                    count = count.saturating_add(1);
                }
            }
        }
        assert!(
            count > 0,
            "稳态 kitty 编码在途时封面区应为 halfblock 真图(品红),不得闪 hash;实得 {count}"
        );
        Ok(())
    }

    /// 全屏稳态切歌转场中途:封面区画两图合成的 halfblock 混色帧(fade 中点 = 均值色),
    /// 而非任一原图 / 程序化色块——证明转场窗口接管了稳态渲染路径。
    #[test]
    fn fullscreen_transition_paints_halfblock_blend() -> color_eyre::Result<()> {
        use std::sync::Arc;

        use mineral_model::MediaUrl;

        use crate::render::anim::Transition;
        use crate::runtime::state::CoverTransition;

        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        let from_url = MediaUrl::remote("https://x.y/from.jpg")?;
        let to_url = MediaUrl::remote("https://x.y/to.jpg")?;
        let solid = |r: u8, g: u8, b: u8| {
            let mut img = image::RgbImage::new(16, 16);
            for p in img.pixels_mut() {
                *p = image::Rgb([r, g, b]);
            }
            Arc::new(image::DynamicImage::ImageRgb8(img))
        };
        app.state.covers.cache.insert(&from_url, solid(200, 0, 0));
        app.state.covers.cache.insert(&to_url, solid(0, 0, 200));
        if let Some(track) = app.state.playback.track.as_mut() {
            track.cover_url = Some(to_url.clone());
        }
        // 转场推到恰好半程(2 拍全程推 1 拍 → 500‰,eased_in_out 过中点仍 500)。
        let mut anim = Transition::expanding(2);
        anim.tick();
        app.state.covers.transition = Some(CoverTransition {
            from_url,
            to_url,
            anim,
        });
        let mut fs = Toggle::new(1);
        fs.set(true);
        fs.tick();
        app.state.browse.fullscreen = fs;

        let mut t = Terminal::new(TestBackend::new(120, 40))?;
        t.draw(|f| super::draw(f, &app))?;
        let blend = Color::Rgb(100, 0, 100);
        let buf = t.backend().buffer();
        let mut count = 0_usize;
        for y in 0..40_u16 {
            for x in 0..120_u16 {
                if buf.cell((x, y)).is_some_and(|c| c.fg == blend) {
                    count = count.saturating_add(1);
                }
            }
        }
        assert!(
            count > 0,
            "转场半程封面区应出现红蓝均值色 halfblock,实得 {count}"
        );
        Ok(())
    }

    /// 全屏稳态 + 在播色板就绪:氛围背景整屏铺 bg——顶行角落(无组件写 bg)应为
    /// 真彩场色且偏离纯底色(封面色可见)。
    #[test]
    fn fullscreen_ambient_paints_background() -> color_eyre::Result<()> {
        use mineral_model::MediaUrl;

        use crate::render::palette::{CoverPalette, Rgb};

        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        let url = MediaUrl::remote("https://x.y/cover.jpg")?;
        if let Some(song) = app.state.player.current.as_mut() {
            song.cover_url = Some(url.clone());
        }
        let pal = CoverPalette::new(vec![Rgb::new(20, 20, 120), Rgb::new(220, 60, 60)])
            .ok_or_else(|| eyre!("非空色板"))?;
        app.state.covers.palettes.insert(url, pal);
        app.sync_cover_palette();
        for _ in 0..400 {
            app.tick_cover_fades();
        }
        let mut fs = Toggle::new(1);
        fs.set(true);
        fs.tick();
        app.state.browse.fullscreen = fs;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        let bg = t
            .backend()
            .buffer()
            .cell((79, 0))
            .ok_or_else(|| eyre!("cell 越界"))?
            .bg;
        assert!(
            matches!(bg, Color::Rgb(..)),
            "全屏顶行角落应铺氛围 bg,实得 {bg:?}"
        );
        assert_ne!(bg, app.theme.base, "场色应偏离纯底色(封面色可见)");
        Ok(())
    }

    /// 全屏稳态 + 无在播色板:氛围场静止在底色——整屏铺出的是纯底色平场
    /// (功能开着就不透出终端默认背景,切歌淡入无缝)。
    #[test]
    fn fullscreen_ambient_without_palette_paints_flat_base() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        let mut fs = Toggle::new(1);
        fs.set(true);
        fs.tick();
        app.state.browse.fullscreen = fs;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        let bg = t
            .backend()
            .buffer()
            .cell((79, 0))
            .ok_or_else(|| eyre!("cell 越界"))?
            .bg;
        assert_eq!(bg, app.theme.base, "无色板时应铺纯底色平场");
        Ok(())
    }

    /// `ambient.enabled = false` 且已静止在底色场:整段跳过铺场,顶行角落 bg 保持
    /// 未写入(终端默认),证明关闭态常态零开销。
    #[test]
    fn fullscreen_ambient_disabled_skips_painting() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        app.apply_pushed_config(mineral_protocol::BusValue::from_json(
            mineral_config::merge_tree(
                mineral_config::default_tree()?,
                serde_json::json!({ "tui": { "ambient": { "enabled": false } } }),
            ),
        ));
        let mut fs = Toggle::new(1);
        fs.set(true);
        fs.tick();
        app.state.browse.fullscreen = fs;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        let bg = t
            .backend()
            .buffer()
            .cell((79, 0))
            .ok_or_else(|| eyre!("cell 越界"))?
            .bg;
        assert_eq!(bg, Color::Reset, "关闭且静止时不应写任何 bg");
        Ok(())
    }

    /// 退出收缩动画中途一帧:边框已向内收,框外清成背景、框内保留界面内容。
    #[test]
    fn quit_shrink_midframe_snapshot() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        // collapsing(18) 推进 12 tick → 收到约 70%,边框明显内收。
        let mut anim = Transition::collapsing(18);
        for _ in 0..12 {
            anim.tick();
        }
        app.transition = Some(anim);

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "退出收缩动画中途:边框内收、框外清空、框内留内容",
            t.backend()
        );
        Ok(())
    }

    /// 启动扩大动画中途一帧:边框由中心向外扩,框外仍清成背景、框内已露界面内容。
    #[test]
    fn startup_expand_midframe_snapshot() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        // expanding(18) 推进 6 tick → 扩到约 30%,边框尚小、由中心向外。
        let mut anim = Transition::expanding(18);
        for _ in 0..6 {
            anim.tick();
        }
        app.transition = Some(anim);

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "启动扩大动画中途:边框由中心外扩、框外清空、框内露内容",
            t.backend()
        );
        Ok(())
    }

    /// 带启动锚点的退出收缩:锚点设在左下角(4, 20),收缩框应偏向该点而非居中
    /// —— 上方/右侧清空区明显大于下方/左侧,验证「朝光标真实位置收」。
    #[test]
    fn collapse_toward_anchor_snapshot() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        app.launch_anchor = Some(Position { x: 4, y: 20 });
        let mut anim = Transition::collapsing(18);
        for _ in 0..12 {
            anim.tick();
        }
        app.transition = Some(anim);

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!("退出收缩朝左下锚点:收缩框偏左下、非居中", t.backend());
        Ok(())
    }

    /// 全屏稳态一帧:只剩 cover(左)/ lyrics(右)/ spectrum + transport(贴底通栏),
    /// 顶栏 / 侧栏 / now_playing 全部退场。
    #[test]
    fn fullscreen_steady_snapshot() -> color_eyre::Result<()> {
        let app = app_in_fullscreen()?;
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "全屏稳态:cover 左 / lyrics 右 / spectrum+transport 贴底",
            t.backend()
        );
        Ok(())
    }

    /// 全屏稳态:下一首(queue 中在播曲的紧邻后继)的封面应被**提前编码**——其 (url, dims)
    /// 进 `covers.encode_pending`。这样自动切歌时协议已就绪、直接 place,不闪程序化占位。
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
                app.state.covers.cache.insert(&url, Arc::new(img));
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
            .covers
            .encode_pending
            .borrow()
            .iter()
            .any(|(u, _)| *u == next_url);
        assert!(warmed, "全屏稳态应提前编码下一首封面");
        Ok(())
    }

    /// Search 布局稳态一帧(无 caps 空态):prompt 行提示无可搜索源、results / detail 仅外框、
    /// transport 全宽贴底;lyrics / spectrum / now_playing 退场。
    #[test]
    fn search_steady_snapshot() -> color_eyre::Result<()> {
        let app = app_with_search()?;
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 稳态空态:prompt 提示无可搜索源 + results/detail 外框 + transport 全宽",
            t.backend()
        );
        Ok(())
    }

    /// 焦点面板边框高亮:prompt 焦点时 prompt 框 accent、results 框 overlay;切 results 焦点反之。
    #[test]
    fn focused_panel_border_is_accent() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        use mineral_model::SearchKind;

        use crate::runtime::state::SearchFocus;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        let accent = app.theme.accent;
        let overlay = app.theme.overlay;
        // prompt 边框左上角在 (0,0)(接管顶行);results 边框左上角在 (0,3)(prompt 占 3 行)。
        let border_fg = |app: &crate::app::App, x: u16, y: u16| -> color_eyre::Result<Color> {
            let mut t = Terminal::new(TestBackend::new(80, 24))?;
            t.draw(|f| super::draw(f, app))?;
            Ok(t.backend()
                .buffer()
                .cell((x, y))
                .ok_or_else(|| eyre!("cell ({x},{y}) 越界"))?
                .fg)
        };

        assert_eq!(
            border_fg(&app, 0, 0)?,
            accent,
            "prompt 焦点 → prompt 框 accent"
        );
        assert_eq!(
            border_fg(&app, 0, 3)?,
            overlay,
            "results 未焦点 → results 框 overlay"
        );

        app.state.channel_search.focus = SearchFocus::Results;
        assert_eq!(
            border_fg(&app, 0, 0)?,
            overlay,
            "prompt 失焦 → prompt 框 overlay"
        );
        assert_eq!(
            border_fg(&app, 0, 3)?,
            accent,
            "results 焦点 → results 框 accent"
        );
        Ok(())
    }

    /// 空结果(尚未搜索)时结果列画**居中 lite 提示**,而非可高亮的列表行:
    /// 结果列 inner 区不应出现任何 surface0 选中高亮带。
    #[test]
    fn empty_results_hint_is_not_highlighted_row() -> color_eyre::Result<()> {
        use crate::runtime::state::SearchFocus;
        use mineral_model::SearchKind;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        // 焦点落结果列(若 hint 被当成可选行,这里就会被高亮——正是要规避的)。
        app.state.channel_search.focus = SearchFocus::Results;
        let surface0 = app.theme.surface0;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        let buf = t.backend().buffer();
        // 结果列在左半(x < 30)、prompt 框(y < 3)之下;只扫结果区,空结果不应有整行底色带
        // (prompt 行的类型徽章本就用 surface0 填底,不在此判定范围)。
        let mut banded = false;
        for y in 3..24u16 {
            for x in 0..30u16 {
                if buf.cell((x, y)).is_some_and(|c| c.bg == surface0) {
                    banded = true;
                }
            }
        }
        assert!(!banded, "空结果的 hint 不应是高亮选中行");
        Ok(())
    }

    /// Search 空结果稳态:结果列画**居中** lite 提示(无可高亮行),prompt / detail 外框照常。
    #[test]
    fn search_empty_results_hint_snapshot() -> color_eyre::Result<()> {
        use mineral_model::SearchKind;

        let (app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 空结果:results 列居中 lite 提示(非高亮行) + prompt/detail 外框",
            t.backend()
        );
        Ok(())
    }

    /// Search 首页在飞:results 列画旋转 spinner「searching」,与「到货 0 条」「尚未搜索」区分。
    #[test]
    fn search_loading_spinner_snapshot() -> color_eyre::Result<()> {
        use mineral_model::SearchKind;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("x");
        }
        // 模拟刚提交首页、结果未到:当前 kind 标在飞。spinner 计数 0 → 首帧 ⠋(无 tick,确定性)。
        app.state.channel_search.mark_loading(SearchKind::Song);
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 首页在飞:results 列居中旋转 spinner「⠋ searching」(区别于空态 / idle)",
            t.backend()
        );
        Ok(())
    }

    /// Search 到货 0 条:results 列画「no results」(bucket 存在但空),区别于 idle 的「type a query」。
    #[test]
    fn search_no_results_hint_snapshot() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("zzzz");
        }
        // 到货 0 条:bucket 建起但空 → no results(apply_page 顺手清 loading)。
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "zzzz".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(Vec::new()),
            has_more: None,
        });
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 到货 0 条:results 列居中「no results」(bucket 在但空,区别于 idle)",
            t.backend()
        );
        Ok(())
    }

    /// 结果列选中行**整行**底色高亮(对齐 tracks/playlist/queue 的 row_highlight,非仅文字变色):
    /// 选中行尾部空白 cell 也带 surface0 底色,非选中行不带。
    #[test]
    fn selected_result_row_has_full_width_highlight() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::SearchFocus;
        use crate::test_support::endserenading;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        app.state.channel_search.focus = SearchFocus::Results;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("x");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "x".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(4)),
            has_more: None,
        });
        // 选中第 2 行(短名 "Gjs · Mineral",尾部留白便于验整行底色)。
        if let Some(kr) = app.state.channel_search.active_results_mut() {
            kr.set_sel(2);
        }
        let surface0 = app.theme.surface0;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        let bg = |x: u16, y: u16| -> color_eyre::Result<Color> {
            Ok(t.backend()
                .buffer()
                .cell((x, y))
                .ok_or_else(|| eyre!("cell ({x},{y}) 越界"))?
                .bg)
        };
        // 结果列起于 y=3(prompt 接管顶行,占 3 行),inner 首行 y=4 是表头,数据行从 y=5 起;
        // 选中第 2 行在 y=7。x=20 是该行尾部留白。
        assert_eq!(bg(20, 7)?, surface0, "选中行尾部空白也应带整行底色");
        assert_ne!(bg(20, 5)?, surface0, "非选中数据行不带底色");
        Ok(())
    }

    /// Search 结果稳态一帧:token prompt 渲染 源徽章 + 类型徽章 + query + 光标,
    /// results 列渲染单曲行(光标行高亮)。
    #[test]
    fn search_results_steady_snapshot() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::test_support::endserenading;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("serenading");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "serenading".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(4)),
            has_more: None,
        });

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 结果稳态:prompt 源/类型徽章 + query + 光标,results 单曲行高亮",
            t.backend()
        );
        Ok(())
    }

    /// Search 形变中途一帧:results / detail 从 sidebar / now_playing 位对飞、prompt 从顶栏下
    /// 长出、lyrics / spectrum 收缩退场。
    #[test]
    fn search_morph_midframe_snapshot() -> color_eyre::Result<()> {
        let mut app = app_with_search()?;
        // 覆盖成形变中途:从零 expanding 推进约半程(9/18 tick)。
        let mut anim = Toggle::new(18);
        anim.set(true);
        for _ in 0..9 {
            anim.tick();
        }
        app.state.channel_search.active = anim;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 形变中途:results/detail 对飞、prompt 长出、lyrics/spectrum 收缩",
            t.backend()
        );
        Ok(())
    }

    /// Search 退场形变中途一帧:从稳态收起,左/右仍画 results/detail 内容随框缩回 browse 位
    /// (而非 esc 瞬间瞬切成 playlists——进退场对称、不在起点跳变)。
    #[test]
    fn search_leave_morph_midframe_snapshot() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::test_support::endserenading;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("serenading");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "serenading".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(4)),
            has_more: None,
        });
        // 推满进场再收起半程(18 拍到顶、再 leave 9 拍):退场中途 on()=false、非端点。
        let mut anim = Toggle::new(18);
        anim.set(true);
        for _ in 0..18 {
            anim.tick();
        }
        anim.set(false);
        for _ in 0..9 {
            anim.tick();
        }
        app.state.channel_search.active = anim;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 退场形变中途:results/detail 随框缩回 browse 位(非起点瞬切 playlists)",
            t.backend()
        );
        Ok(())
    }

    /// 非焦点结果列的选中行走**暗调高亮**(subtext,非 accent):焦点在 prompt 时仍标出
    /// "回得去"的光标位置而不抢视觉;切回结果列焦点则恢复 accent 亮高亮。
    #[test]
    fn unfocused_results_row_uses_dim_highlight() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::SearchFocus;
        use crate::test_support::endserenading;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("serenading");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "serenading".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(4)),
            has_more: None,
        });
        if let Some(kr) = app.state.channel_search.active_results_mut() {
            kr.set_sel(2);
        }
        let accent = app.theme.accent;
        let subtext = app.theme.subtext;
        // 选中第 2 行 → inner y=7(prompt 3 + 边框 1 + 表头 1 + 2);x=20 是该行(整行高亮)。
        let row_fg = |app: &crate::app::App| -> color_eyre::Result<Color> {
            let mut t = Terminal::new(TestBackend::new(80, 24))?;
            t.draw(|f| super::draw(f, app))?;
            Ok(t.backend()
                .buffer()
                .cell((20, 7))
                .ok_or_else(|| eyre!("cell (20,7) 越界"))?
                .fg)
        };

        // 默认焦点在 prompt:结果列选中行走暗调(subtext)。
        assert_eq!(row_fg(&app)?, subtext, "非焦点结果列 → 暗调高亮(subtext)");
        app.state.channel_search.focus = SearchFocus::Results;
        assert_eq!(row_fg(&app)?, accent, "焦点结果列 → accent 亮高亮");
        Ok(())
    }

    /// Search 焦点环滑动中途一帧:焦点 prompt→results,accent 浮动边框悬在两面板矩形之间,
    /// 两面板自身边框压暗(高亮交给浮动环)。
    #[test]
    fn search_focus_ring_slide_midframe_snapshot() -> color_eyre::Result<()> {
        use mineral_model::SearchKind;

        use crate::render::anim::Transition;
        use crate::runtime::state::SearchFocus;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        app.state.channel_search.set_focus(SearchFocus::Results);
        // 焦点环推进约半程(9/18),环悬在 prompt 与 results 矩形之间。
        let mut ring = Transition::expanding(18);
        for _ in 0..9 {
            ring.tick();
        }
        app.state.channel_search.focus_ring = ring;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 焦点环滑动中途:accent 浮动边框悬在 prompt 与 results 之间,两面板边框压暗",
            t.backend()
        );
        Ok(())
    }

    /// Search detail 焦点稳态:detail 框 accent 高亮、results 框压暗且选中行走暗调高亮。
    #[test]
    fn search_detail_focused_snapshot() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::SearchFocus;
        use crate::test_support::endserenading;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("serenading");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "serenading".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(4)),
            has_more: None,
        });
        // 直接置焦点 detail(环 settle,不滑动):detail 高亮、results 暗调。
        app.state.channel_search.focus = SearchFocus::Detail;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search detail 焦点:detail 框 accent + results 框暗调 + 选中行暗调高亮",
            t.backend()
        );
        Ok(())
    }

    /// Search 歌单 detail 曲目表:对齐 browse library 风格 —— ♥/#/title/artist/album/len 六列 +
    /// 表头,在播歌 # 列显 ♫、收藏歌显 ♥。宽 120 让 detail 面板够 Full 档(artist/album 不退)。
    #[test]
    fn search_detail_playlist_tracks_snapshot() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{
            AlbumId, AlbumRef, ArtistId, ArtistRef, Playlist, PlaylistId, SearchKind, Song, SongId,
            SourceKind,
        };
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::SearchFocus;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Playlist])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("emo");
        }
        let track = |id: &str, name: &str, artist: &str, album: &str, dur: u64| {
            Song::builder()
                .id(SongId::new(SourceKind::NETEASE, id))
                .name(name.to_owned())
                .artists(vec![ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, id),
                    name: artist.to_owned(),
                }])
                .album(Some(AlbumRef {
                    id: AlbumId::new(SourceKind::NETEASE, id),
                    name: album.to_owned(),
                }))
                .duration_ms(Some(dur))
                .build()
        };
        let songs = vec![
            track("1", "Endserenading", "Mineral", "EndSerenading", 225_000),
            track(
                "2",
                "守门员",
                "Chinese Football",
                "Chinese Football",
                233_000,
            ),
            track("3", "Palisade", "Mineral", "EndSerenading", 271_000),
        ];
        let playlist = Playlist::builder()
            .id(PlaylistId::new(SourceKind::NETEASE, "pl1"))
            .name("emo mix".to_owned())
            .track_count(3)
            .build();
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Playlist,
            query: "emo".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Playlists(vec![playlist]),
            has_more: None,
        });
        // 歌单 root 帧补拉到曲目(DetailData::Tracks)。
        if let Some(kr) = app.state.channel_search.active_results_mut()
            && let Some(frame) = kr.detail.current_mut()
        {
            frame.set_tracks(songs.clone());
        }
        // 第 1 首在播(♫)、第 2 首已收藏(♥)。
        app.state.player.current = songs.first().cloned();
        let liked = songs
            .get(1)
            .ok_or_else(|| eyre!("fixture 应有第 2 首"))?
            .clone();
        app.state.toggle_loved_local(&liked);
        app.state.channel_search.focus = SearchFocus::Detail;

        let mut t = Terminal::new(TestBackend::new(120, 26))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 歌单 detail 曲目表:♥/#/title/artist/album/len 六列 + 表头(♫ 在播 / ♥ 收藏)",
            t.backend()
        );
        Ok(())
    }

    /// 造一个带热门曲 + 专辑列表的 artist detail 帧(测试 helper)：搜索 artist → root 帧补拉
    /// detail(热门曲) + albums(带 track_count/发行年/厂牌)。返回已置焦 detail 的 App。
    fn app_with_artist_detail() -> color_eyre::Result<crate::app::App> {
        use mineral_channel_core::Page;
        use mineral_model::{
            Album, AlbumId, AlbumRef, Artist, ArtistId, ArtistRef, SearchKind, Song, SongId,
            SourceKind,
        };
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::SearchFocus;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Artist])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("mineral");
        }
        let result_artist = Artist::builder()
            .id(ArtistId::new(SourceKind::NETEASE, "ar"))
            .name("Mineral".to_owned())
            .follower_count(Some(176_393))
            .build();
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Artist,
            query: "mineral".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Artists(vec![result_artist]),
            has_more: None,
        });
        let hot = |id: &str, name: &str, album: &str, dur: u64| {
            Song::builder()
                .id(SongId::new(SourceKind::NETEASE, id))
                .name(name.to_owned())
                .artists(vec![ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, "ar"),
                    name: "Mineral".to_owned(),
                }])
                .album(Some(AlbumRef {
                    id: AlbumId::new(SourceKind::NETEASE, "al1"),
                    name: album.to_owned(),
                }))
                .duration_ms(Some(dur))
                .build()
        };
        let detail = Artist::builder()
            .id(ArtistId::new(SourceKind::NETEASE, "ar"))
            .name("Mineral".to_owned())
            .follower_count(Some(176_393))
            .album_count(Some(2))
            .song_count(Some(21))
            .description("Texas emo, 1994–1998".to_owned())
            .songs(vec![
                hot("1", "LoveLetterTypewriter", "EndSerenading", 225_000),
                hot("2", "Palisade", "EndSerenading", 271_000),
            ])
            .build();
        let album = |id: &str, name: &str, n: u64, ms: i64, co: &str| {
            Album::builder()
                .id(AlbumId::new(SourceKind::NETEASE, id))
                .name(name.to_owned())
                .track_count(Some(n))
                .publish_time_ms(ms)
                .company(Some(co.to_owned()))
                .build()
        };
        let albums = vec![
            album(
                "al1",
                "EndSerenading",
                10,
                1_443_196_800_000,
                "Crank! Records",
            ),
            album(
                "al2",
                "ThePowerOfFailing",
                11,
                1_577_808_000_000,
                "Crank! Records",
            ),
        ];
        if let Some(kr) = app.state.channel_search.active_results_mut()
            && let Some(frame) = kr.detail.current_mut()
        {
            frame.set_artist_detail(Box::new(detail));
            frame.set_artist_albums(albums);
        }
        app.state.channel_search.focus = SearchFocus::Detail;
        Ok(app)
    }

    /// Search artist detail 的 Albums 区:专辑表 name/tracks/year/label 四列 + 表头。
    #[test]
    fn search_detail_artist_albums_snapshot() -> color_eyre::Result<()> {
        use crate::runtime::state::ArtistSection;

        let mut app = app_with_artist_detail()?;
        if let Some(kr) = app.state.channel_search.active_results_mut()
            && let Some(frame) = kr.detail.current_mut()
        {
            frame.section = ArtistSection::Albums;
        }
        let mut t = Terminal::new(TestBackend::new(120, 26))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search artist detail Albums 区:专辑表 name/tracks/year/label 四列 + 表头",
            t.backend()
        );
        Ok(())
    }

    /// Search artist detail 双区切换中途一帧:Top Songs 与 Albums 列表横向合成(尊重 view_sweep)。
    #[test]
    fn search_detail_section_sweep_midframe_snapshot() -> color_eyre::Result<()> {
        let mut app = app_with_artist_detail()?;
        if let Some(kr) = app.state.channel_search.active_results_mut() {
            if let Some(frame) = kr.detail.current_mut() {
                frame.cycle_section(/*ticks*/ 8);
            }
            // 推进约 3/8:既非起点也非终点,两区列表同屏合成。
            for _ in 0..3 {
                kr.detail.tick();
            }
        }
        let mut t = Terminal::new(TestBackend::new(120, 26))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search artist detail 切区中途:Top Songs↔Albums 横向合成(列表区,Tab/头图不滑)",
            t.backend()
        );
        Ok(())
    }

    /// 造一个带多段简介(含 \n\n 段落 + \n 制作名单)的专辑 detail 帧(测试 helper)：
    /// 搜专辑 → root 帧补拉完整 detail(含 description)。返回已置焦 detail 的 App。
    fn app_with_album_description() -> color_eyre::Result<crate::app::App> {
        use mineral_channel_core::Page;
        use mineral_model::{Album, AlbumId, SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::SearchFocus;
        use crate::test_support::endserenading;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Album])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("winlose");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "winlose".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![
                Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("Win&Lose".to_owned())
                    .build(),
            ]),
            has_more: None,
        });
        // 仿网易云原文:段落用 \n\n、制作名单用 \n。换行必须渲染成多行(非塞进一个 Line);
        // 长到溢出简介区,触发滚动条 + 让滚动演示有意义。
        let desc = "三年过去，还是没能成为一个厉害的大人。Chinese Football也没能成为一个摇滚天团。\n\n游戏一旦开始就注定会有一个结局。\n胜利或失败。\n\n每个人都想成为赢家，用胜利的喜悦去回报付出的时间。\n\n只是偶尔还会发梦，梦到还未走向最终的结局。（文/徐波）\n\n发行厂牌：野生唱片\n发行编号：WILD-022\n录音师：骷髅，李珂\n混音/母带：骷髅\n制作：Chinese Football\n封面插画：史悲";
        app.state.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            album: Box::new(
                Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("Win&Lose".to_owned())
                    .track_count(Some(12))
                    .publish_time_ms(1_672_329_600_000)
                    .company(Some("野生唱片".to_owned()))
                    .description(desc.to_owned())
                    .songs(endserenading(3))
                    .build(),
            ),
        });
        app.state.channel_search.focus = SearchFocus::Detail;
        Ok(app)
    }

    /// Search 专辑 detail 头部简介:网易云多段原文按 \n 渲染成多行(修「整段塞一个 Line、换行
    /// 被吞」),header 下独立简介区,溢出末列画滚动条。
    #[test]
    fn search_detail_album_description_snapshot() -> color_eyre::Result<()> {
        let app = app_with_album_description()?;
        // 高 40:让 head 简介区有多行,既展示多段换行又仍溢出(滚动条可见)。
        let mut t = Terminal::new(TestBackend::new(120, 40))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 专辑 detail 简介:多段原文按 \\n 多行渲染 + 溢出滚动条",
            t.backend()
        );
        Ok(())
    }

    /// Search 专辑 detail 简介滚到底:窗口落在简介尾部(header 不动),滚动条滑块底边贴轨道底
    /// (回归用户实测——到底了滑块却没到底的误导)。nudge 一个大值,render 端 clamp 到 max offset。
    #[test]
    fn search_detail_description_scrolled_snapshot() -> color_eyre::Result<()> {
        let app = app_with_album_description()?;
        if let Some(frame) = app
            .state
            .channel_search
            .active_results()
            .and_then(|kr| kr.detail.current())
        {
            frame.nudge_description(/*delta*/ 1000);
        }
        let mut t = Terminal::new(TestBackend::new(120, 40))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 专辑 detail 简介滚到底:窗口在尾部 + 滚动条滑块贴轨道底",
            t.backend()
        );
        Ok(())
    }

    /// 专辑结果:结果列按类型走「专辑名 · 艺人」两列对齐(非纯单行文字)。
    #[test]
    fn search_results_albums_snapshot() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{Album, AlbumId, ArtistId, ArtistRef, SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Album])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("mineral");
        }
        let album = |id: &str, name: &str, who: &str| {
            Album::builder()
                .id(AlbumId::new(SourceKind::NETEASE, id))
                .name(name.to_owned())
                .artists(vec![ArtistRef {
                    id: ArtistId::new(SourceKind::NETEASE, id),
                    name: who.to_owned(),
                }])
                .build()
        };
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "mineral".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![
                album("1", "Power", "Mineral"),
                album("2", "EndSerenading", "Mineral"),
            ]),
            has_more: None,
        });
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!("Search 专辑结果:专辑名 · 艺人 两列对齐", t.backend());
        Ok(())
    }

    /// 歌单结果:结果列走「歌单名 · N tracks」两列对齐。
    #[test]
    fn search_results_playlists_snapshot() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{Playlist, PlaylistId, SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Playlist])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("indie");
        }
        let playlist = |id: &str, name: &str, count: u64| {
            Playlist::builder()
                .id(PlaylistId::new(SourceKind::NETEASE, id))
                .name(name.to_owned())
                .track_count(count)
                .build()
        };
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Playlist,
            query: "indie".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Playlists(vec![
                playlist("1", "Bedroom Pop", 42),
                playlist("2", "Math Rock 精选", 128),
            ]),
            has_more: None,
        });
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 歌单结果:歌单名 · N tracks 两列对齐",
            t.backend()
        );
        Ok(())
    }

    /// artist 结果:结果列走「artist 名 · 关注数缩写」两列对齐(humanize:42k / 1M)。
    #[test]
    fn search_results_artists_snapshot() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{Artist, ArtistId, SearchKind, SourceKind};
        use mineral_task::{SearchPayload, TaskEvent};

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Artist])?;
        if let Some(session) = app.state.channel_search.current_mut() {
            session.set_query("football");
        }
        let artist = |id: &str, name: &str, followers: u64| {
            Artist::builder()
                .id(ArtistId::new(SourceKind::NETEASE, id))
                .name(name.to_owned())
                .follower_count(Some(followers))
                .build()
        };
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Artist,
            query: "football".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Artists(vec![
                artist("1", "Chinese Football", 42_000),
                artist("2", "American Football", 1_500_000),
            ]),
            has_more: None,
        });
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search artist 结果:artist 名 · 关注数缩写 两列对齐",
            t.backend()
        );
        Ok(())
    }

    /// 全屏形变中途一帧:封面右→左、歌词左→右对穿,消失面板收缩中。
    #[test]
    fn fullscreen_morph_midframe_snapshot() -> color_eyre::Result<()> {
        let mut app = app_in_fullscreen()?;
        // 覆盖成形变中途:expanding 推进 9 tick(约半程,未到满)。
        let mut anim = Toggle::new(18);
        anim.set(true);
        for _ in 0..9 {
            anim.tick();
        }
        app.state.browse.fullscreen = anim;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "全屏形变中途:封面右→左、歌词左→右对穿、消失面板收缩",
            t.backend()
        );
        Ok(())
    }
}

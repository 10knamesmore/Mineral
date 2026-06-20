//! 主帧渲染入口。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use mineral_config::SearchFocusTransition;

use crate::app::App;
use crate::components::layout::compute::{Areas, compute, compute_fullscreen, compute_search};
use crate::components::layout::{
    cover, cover_image, lyrics, now_playing, search_detail, search_panel, sidebar, spectrum,
    top_status, transform, transport,
};
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
        let areas = if app.state.browse.fullscreen.at_max() {
            full
        } else {
            transform::morph_areas(&normal, &full, app.state.browse.fullscreen.eased_in_out())
        };
        paint_fullscreen(frame, &areas, app);
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
    transport::draw(frame, areas.transport, &app.state.playback, theme);
}

/// Search 布局:顶栏 + prompt 行 + results(左)/ detail(右)面板 + transport 全宽。
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
    top_status::draw(frame, areas.top_status, &app.state, theme);
    if let Some(prompt) = areas.search_prompt {
        search_panel::draw_prompt(
            frame,
            prompt,
            rs,
            theme,
            border_focused(SearchFocus::Prompt),
        );
    }
    if let Some(left) = nonempty(areas.left) {
        if show_browse {
            sidebar::draw(frame, left, &app.state, theme);
        } else {
            search_panel::draw_results(
                frame,
                left,
                rs,
                theme,
                border_focused(SearchFocus::Results),
            );
        }
    }
    if let Some(right) = areas.right.and_then(nonempty) {
        if show_browse {
            now_playing::draw(frame, right, &app.state, &app.picker, theme);
        } else {
            search_detail::draw(
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
        search_panel::draw_prompt_dropdown(frame, prompt, &app.state, theme);
    }
    transport::draw(frame, areas.transport, &app.state.playback, theme);
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

/// 全屏 / 形变布局:消失面板渲进收缩 rect(小到自动空白)→ spectrum → transport → cover
/// → lyrics(歌词最后画,对穿交错时压在封面上保持可读)。
fn paint_fullscreen(frame: &mut Frame<'_>, areas: &Areas, app: &App) {
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
    transport::draw(frame, areas.transport, &app.state.playback, theme);
    if let Some(c) = areas.cover.and_then(nonempty) {
        draw_fullscreen_cover(frame, c, app);
    }
    if let Some(lyr) = areas.lyrics.and_then(nonempty) {
        lyrics::draw(frame, lyr, &app.state, theme, lyrics::LyricMode::Immersive);
    }
}

/// 全屏独立封面:跟**在播曲**;形变中画程序化色块(便宜),稳态全屏才上真图(避免形变期
/// 每帧尺寸变导致 protocol 重编码)。无在播曲时叠居中 `暂无播放` 提示。
fn draw_fullscreen_cover(frame: &mut Frame<'_>, area: Rect, app: &App) {
    let theme = &app.theme;
    let track = app.state.playback.track.as_ref();
    let seed = track.map_or_else(
        || "mineral".to_owned(),
        |t| {
            t.album
                .as_ref()
                .map_or_else(|| t.name.clone(), |a| a.name.clone())
        },
    );
    if app.state.browse.fullscreen.at_max() {
        cover_image::render_or_fallback(
            frame,
            area,
            track.and_then(|t| t.cover_url.as_ref()),
            &app.state,
            &app.picker,
            theme,
            &seed,
        );
        // 全屏稳态封面区尺寸固定:顺手把后续若干首按同尺寸提前编码,自动切歌时协议已就绪、
        // 直接 place,消掉切歌瞬间的程序化占位闪。形变期(非 at_max)绝不预热——那会按
        // 逐帧漂移 dims 编码(churn)。
        prewarm_upcoming(app, area);
    } else {
        cover::render(frame, area, &seed, theme);
    }
    if track.is_none() {
        let y = area.y + area.height / 2;
        let strip = Rect::new(area.x, y, area.width, 1);
        let line = Line::from("nothing playing").style(
            Style::new()
                .fg(theme.overlay)
                .add_modifier(Modifier::ITALIC),
        );
        frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), strip);
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
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;
    use ratatui::style::Color;

    use crate::render::anim::{Toggle, Transition};
    use crate::test_support::{app_in_fullscreen, app_with_queue, app_with_search};

    /// 回归:全屏形变期间,正在收缩的 now_playing 面板**不得**派发封面编码请求。
    ///
    /// 封面编码已离线(投递给 `CoverEncoder` worker);若形变中逐帧 now_playing 尺寸变
    /// 还照常派发,会按逐帧漂移的 dims **flood 编码器**(churn,且稳态落地后占位符乱套留
    /// 残影)。修复:形变期(`!settled`)`cover_image` 早退,不派发。这里验证整段形变中
    /// `covers.encode_pending` 不新增——只保留稳态那次派发。
    #[test]
    fn fullscreen_morph_does_not_dispatch_cover_encode() -> color_eyre::Result<()> {
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
        app.state.covers.cache.insert(url.clone(), Arc::new(img));
        // 关掉滚动防抖早退(置选中变化于防抖窗口之外),让稳态帧真正派发一次编码。
        app.state.browse.nav.last_sel_change = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);

        let mut t = Terminal::new(TestBackend::new(120, 40))?;

        // 稳态老布局:渲一帧 → 派发一次封面编码(稳态 dims)。记录此刻 pending 快照。
        t.draw(|f| super::draw(f, &app))?;
        let steady_pending = app.state.covers.encode_pending.borrow().clone();
        assert_eq!(steady_pending.len(), 1, "稳态老布局应恰好派发一次封面编码");

        // 进入全屏,推进若干形变帧(均 `!settled`)。每帧后 pending 必须与稳态快照一致 ——
        // 证明 now_playing 消失面板没有在形变中按漂移 dims 追加派发。
        app.state.browse.fullscreen.set(true);
        for _ in 0..5 {
            app.state.browse.fullscreen.tick();
            assert!(
                !app.state.browse.fullscreen.settled(),
                "测试需停留在形变中途"
            );
            t.draw(|f| super::draw(f, &app))?;
            assert_eq!(
                *app.state.covers.encode_pending.borrow(),
                steady_pending,
                "形变期不应追加封面编码派发(churn)"
            );
        }
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
                app.state.covers.cache.insert(url, Arc::new(img));
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
        // prompt 边框左上角在 (0,1)(顶栏 1 行之下);results 边框左上角在 (0,4)(prompt 占 3 行)。
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
            border_fg(&app, 0, 1)?,
            accent,
            "prompt 焦点 → prompt 框 accent"
        );
        assert_eq!(
            border_fg(&app, 0, 4)?,
            overlay,
            "results 未焦点 → results 框 overlay"
        );

        app.state.channel_search.focus = SearchFocus::Results;
        assert_eq!(
            border_fg(&app, 0, 1)?,
            overlay,
            "prompt 失焦 → prompt 框 overlay"
        );
        assert_eq!(
            border_fg(&app, 0, 4)?,
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
        // 结果列在左半(x < 30)、prompt 行(y < 4)之下;只扫结果区,空结果不应有整行底色带
        // (prompt 行的类型徽章本就用 surface0 填底,不在此判定范围)。
        let mut banded = false;
        for y in 4..24u16 {
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
        });
        // 选中第 2 行(短名 "Gjs · Mineral",尾部留白便于验整行底色)。
        if let Some(kr) = app.state.channel_search.active_results_mut() {
            kr.sel = 2;
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
        // 结果列起于 y=4(顶栏 1 + prompt 3),inner 首行 y=5 是表头,数据行从 y=6 起;
        // 选中第 2 行在 y=8。x=20 是该行尾部留白。
        assert_eq!(bg(20, 8)?, surface0, "选中行尾部空白也应带整行底色");
        assert_ne!(bg(20, 6)?, surface0, "非选中数据行不带底色");
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
        });
        if let Some(kr) = app.state.channel_search.active_results_mut() {
            kr.sel = 2;
        }
        let accent = app.theme.accent;
        let subtext = app.theme.subtext;
        // 选中第 2 行 → inner y=8(顶栏 1 + prompt 3 + 边框 1 + 表头 1 + 2);x=20 是该行(整行高亮)。
        let row_fg = |app: &crate::app::App| -> color_eyre::Result<Color> {
            let mut t = Terminal::new(TestBackend::new(80, 24))?;
            t.draw(|f| super::draw(f, app))?;
            Ok(t.backend()
                .buffer()
                .cell((20, 8))
                .ok_or_else(|| eyre!("cell (20,8) 越界"))?
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
        });
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 歌单结果:歌单名 · N tracks 两列对齐",
            t.backend()
        );
        Ok(())
    }

    /// 歌手结果:结果列走「歌手名 · 关注数缩写」两列对齐(humanize:42k / 1M)。
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
                .follower_count(followers)
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
        });
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 歌手结果:歌手名 · 关注数缩写 两列对齐",
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

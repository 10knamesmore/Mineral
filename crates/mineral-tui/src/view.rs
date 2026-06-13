//! 主帧渲染入口。

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::app::App;
use crate::components::layout::compute::{Areas, compute, compute_fullscreen, compute_search};
use crate::components::layout::{
    cover, cover_image, lyrics, now_playing, sidebar, spectrum, top_status, transform, transport,
};

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
    if !app.state.fullscreen.at_min() {
        let full = compute_fullscreen(frame.area(), layout_cfg);
        let areas = if app.state.fullscreen.at_max() {
            full
        } else {
            transform::morph_areas(&normal, &full, app.state.fullscreen.eased_in_out())
        };
        paint_fullscreen(frame, &areas, app);
    } else if !app.state.search.remote_search.active.at_min() {
        let search = compute_search(frame.area(), layout_cfg);
        let areas = if app.state.search.remote_search.active.at_max() {
            search
        } else {
            transform::morph_search(
                &normal,
                &search,
                app.state.search.remote_search.active.eased_in_out(),
            )
        };
        paint_search(frame, &areas, app);
    } else {
        paint(frame, &normal, app);
    }

    // topbar 通知层 / 浮层栈:整屏转场(启动扩大 / 退出收缩)期间不画;全屏形变不抑制。
    // 通知锚点恒用常规顶栏行(全屏顶栏已收掉,仍从屏顶向下堆叠)。沉浸进度直接喂
    // 形变缓动值:z 切换期间通知锚点随布局连续插值(居中 ↔ 右上),不瞬移。
    if app.transition.is_none() {
        app.notifications.render(
            frame,
            normal.top_status,
            theme,
            app.state.fullscreen.eased_in_out(),
            &app.notice_hint,
        );
        app.overlays.render(frame, frame.area(), &app.state, theme);
    }

    if let Some(anim) = &app.transition {
        transform::clip_scaled(frame, frame.area(), anim.eased(), app.launch_anchor, theme);
    }
}

/// 常规(浏览态)布局:把各 area 分发给对应组件渲染。
fn paint(frame: &mut Frame<'_>, areas: &Areas, app: &App) {
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

/// Search 布局:顶栏 + prompt 行 + results(左)/ detail(右)占位面板 + transport 全宽。
/// 形变中途额外画收缩中的 lyrics / spectrum(浏览内容,稳态端点为 None 自动跳过)。面板内容
/// 画在给定 rect 内——端点或飞行中皆然。真实 token prompt / 结果列 / 详情随后续里程碑填入。
fn paint_search(frame: &mut Frame<'_>, areas: &Areas, app: &App) {
    let theme = &app.theme;
    top_status::draw(frame, areas.top_status, &app.state, theme);
    if let Some(prompt) = areas.search_prompt {
        // 空查询占位:放大镜图标(标准 unicode,单宽);真 token prompt 后续里程碑接入。
        let hint = Line::from("⌕").style(Style::new().fg(theme.overlay));
        frame.render_widget(Paragraph::new(hint), prompt);
    }
    let border = Style::new().fg(theme.overlay);
    if let Some(results) = nonempty(areas.left) {
        frame.render_widget(
            Block::new()
                .borders(Borders::ALL)
                .border_style(border)
                .title("results"),
            results,
        );
    }
    if let Some(detail) = areas.right.and_then(nonempty) {
        frame.render_widget(
            Block::new()
                .borders(Borders::ALL)
                .border_style(border)
                .title("detail"),
            detail,
        );
    }
    // 形变期收缩中的 lyrics / spectrum 仍画浏览内容(稳态 search 端点为 None,自动跳过)。
    if let Some(lyr) = areas.lyrics.and_then(nonempty) {
        lyrics::draw(frame, lyr, &app.state, theme, lyrics::LyricMode::Compact);
    }
    if let Some(spec) = areas.spectrum.and_then(nonempty) {
        spectrum::draw(frame, spec, &app.state.spectrum, theme);
    }
    transport::draw(frame, areas.transport, &app.state.playback, theme);
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
    if app.state.fullscreen.at_max() {
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
        let line = Line::from("暂无播放").style(
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
        app.state.nav.last_sel_change = Instant::now()
            .checked_sub(Duration::from_secs(1))
            .unwrap_or_else(Instant::now);

        let mut t = Terminal::new(TestBackend::new(120, 40))?;

        // 稳态老布局:渲一帧 → 派发一次封面编码(稳态 dims)。记录此刻 pending 快照。
        t.draw(|f| super::draw(f, &app))?;
        let steady_pending = app.state.covers.encode_pending.borrow().clone();
        assert_eq!(steady_pending.len(), 1, "稳态老布局应恰好派发一次封面编码");

        // 进入全屏,推进若干形变帧(均 `!settled`)。每帧后 pending 必须与稳态快照一致 ——
        // 证明 now_playing 消失面板没有在形变中按漂移 dims 追加派发。
        app.state.fullscreen.set(true);
        for _ in 0..5 {
            app.state.fullscreen.tick();
            assert!(!app.state.fullscreen.settled(), "测试需停留在形变中途");
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
        app.state.fullscreen = fs;

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

    /// Search 布局稳态一帧:顶栏保留、prompt 行 + results(左)/ detail(右)空占位面板、
    /// transport 全宽贴底;lyrics / spectrum / now_playing 退场。
    #[test]
    fn search_steady_snapshot() -> color_eyre::Result<()> {
        let app = app_with_search()?;
        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 稳态:prompt 行 + results 左 / detail 右占位 + transport 全宽",
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
        app.state.search.remote_search.active = anim;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "Search 形变中途:results/detail 对飞、prompt 长出、lyrics/spectrum 收缩",
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
        app.state.fullscreen = anim;

        let mut t = Terminal::new(TestBackend::new(80, 24))?;
        t.draw(|f| super::draw(f, &app))?;
        crate::test_support::assert_snap!(
            "全屏形变中途:封面右→左、歌词左→右对穿、消失面板收缩",
            t.backend()
        );
        Ok(())
    }
}

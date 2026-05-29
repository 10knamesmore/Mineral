//! 主帧渲染入口。

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear};

use crate::app::App;
use crate::components::{lyrics, now_playing, sidebar, spectrum, top_status, transport};
use crate::layout::{Areas, compute};
use crate::theme::Theme;

/// 收缩进度满值(千分比),对齐 [`crate::anim::Transition::eased`] 的满值。
const FULL_SCALE: u32 = 1000;

/// 渲染一帧:计算布局,填充各面板;整屏转场(启动扩大 / 退出收缩)进行时叠加缩放边框。
pub fn draw(frame: &mut Frame<'_>, app: &App) {
    let areas = compute(frame.area());
    paint(frame, &areas, app);
    if let Some(anim) = &app.transition {
        clip_scaled(
            frame,
            frame.area(),
            anim.eased(),
            app.launch_anchor,
            &app.theme,
        );
    }
}

/// 把 layout 计算出的各 area 分发给对应组件渲染。
fn paint(frame: &mut Frame<'_>, areas: &Areas, app: &App) {
    let theme = &app.theme;
    top_status::draw(frame, areas.top_status, &app.state, theme);
    sidebar::draw(frame, areas.left, &app.state, theme);
    if let Some(right) = areas.right {
        now_playing::draw(frame, right, &app.state, &app.picker, theme);
    }
    if let Some(lyr) = areas.lyrics {
        lyrics::draw(frame, lyr, &app.state, theme);
    }
    if let Some(spec) = areas.spectrum {
        spectrum::draw(frame, spec, &app.state.spectrum, theme);
    }
    transport::draw(frame, areas.transport, &app.state.playback, theme);

    // topbar 通知层(下载进度 / 一次性消息):自 top_status 行起向下堆叠,top-center 展开。转场期间不画。
    if app.transition.is_none() {
        app.notifications.render(frame, areas.top_status, theme);
    }

    // 浮层栈:整屏转场动画期间不画(此时缩放的是主界面整体,不带浮层)。
    if app.transition.is_none() {
        app.overlays.render(frame, frame.area(), &app.state, theme);
    }
}

/// 整屏缩放裁剪:一个整 cell 边框在 `area` 内按 `scale`(千分比)朝 `anchor` 缩放,框内保留
/// 已画的界面内容(静止),框外清成背景色。`scale` 由 0 涨到满即启动扩大、由满收到 0 即退出
/// 收缩 —— 方向只取决于 `scale` 的走向,渲染本身方向无关。
///
/// `anchor` 为进 alternate screen 前捕获的光标位置:expand 从此点铺开、collapse 收回此点。
/// `None`(无 TTY)时退化回屏幕居中,即历史行为。
fn clip_scaled(
    frame: &mut Frame<'_>,
    area: Rect,
    scale: u16,
    anchor: Option<Position>,
    theme: &Theme,
) {
    let scale = u32::from(scale);
    let w = u16::try_from(u32::from(area.width) * scale / FULL_SCALE).unwrap_or(area.width);
    let h = u16::try_from(u32::from(area.height) * scale / FULL_SCALE).unwrap_or(area.height);

    let area_bottom = area.y.saturating_add(area.height);
    let area_right = area.x.saturating_add(area.width);
    // 框左上角定位:有锚点则朝锚点插值,无锚点(无 TTY)退化回居中。
    // 居中分支逐字节沿用历史公式(`(area - 框) / 2`),与定点取整顺序一致 → 历史快照不变;
    // 锚点分支才走新插值,把改动收窄到「确有光标位置」时。
    let (x, y) = match anchor {
        Some(p) => {
            // clamp 进 area(防 enter 后终端缩小致 row/col 越界);`.max(area.{x,y})` 保证
            // clamp 的 min ≤ max(空 area 时也不 panic)。
            let ax = p.x.clamp(area.x, area_right.saturating_sub(1).max(area.x));
            let ay = p.y.clamp(area.y, area_bottom.saturating_sub(1).max(area.y));
            // scale=满 → 贴 area 原点(全屏);scale=0 → 收到锚点(成一点)。锚点在框内
            // 相对比例恒定 → 真正"从该点放 / 朝该点收"。全程非负定点,不碰 as 强转。
            let dx = u32::from(ax - area.x) * (FULL_SCALE - scale) / FULL_SCALE;
            let dy = u32::from(ay - area.y) * (FULL_SCALE - scale) / FULL_SCALE;
            (
                area.x.saturating_add(u16::try_from(dx).unwrap_or(0)),
                area.y.saturating_add(u16::try_from(dy).unwrap_or(0)),
            )
        }
        None => (
            area.x + area.width.saturating_sub(w) / 2,
            area.y + area.height.saturating_sub(h) / 2,
        ),
    };

    // 框外四条 bar 清成背景色,把界面"吞掉"。
    let below = y.saturating_add(h);
    let right = x.saturating_add(w);
    fill_bg(
        frame,
        Rect::new(area.x, area.y, area.width, y.saturating_sub(area.y)),
        theme.base,
    );
    fill_bg(
        frame,
        Rect::new(area.x, below, area.width, area_bottom.saturating_sub(below)),
        theme.base,
    );
    fill_bg(
        frame,
        Rect::new(area.x, y, x.saturating_sub(area.x), h),
        theme.base,
    );
    fill_bg(
        frame,
        Rect::new(right, y, area_right.saturating_sub(right), h),
        theme.base,
    );

    // 收缩边框(只画线,不填内部 → 保留框内内容);收到太小画不出框就只剩清屏。
    if w >= 2 && h >= 2 {
        let block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme.accent));
        frame.render_widget(block, Rect::new(x, y, w, h));
    }
}

/// 把 `rect` 清成纯 `color` 背景(先 `Clear` 去掉旧字符,再铺底色)。空矩形跳过。
fn fill_bg(frame: &mut Frame<'_>, rect: Rect, color: Color) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    frame.render_widget(Clear, rect);
    frame.render_widget(Block::new().style(Style::new().bg(color)), rect);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Position;

    use crate::anim::Transition;
    use crate::test_support::app_with_queue;

    /// 退出收缩动画中途一帧:边框已向内收,框外清成背景、框内保留界面内容。
    #[test]
    fn quit_shrink_midframe_snapshot() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0);
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
        let mut app = app_with_queue(3, /*current_idx*/ 0);
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
        let mut app = app_with_queue(3, /*current_idx*/ 0);
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
}

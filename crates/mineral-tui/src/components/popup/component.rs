//! 浮层基础组件:统一的 [`Overlay`] 抽象。
//!
//! chrome 自动提供居中 layout + 中心缩放弹出动画;实现方只声明四件事(外框尺寸、
//! 外框 Block、内容渲染、按键响应),既不持有动画状态(由 stack 托管 [`Transition`]),
//! 也不直接操作 App —— 按键产出 [`OverlayAction`] 回传执行,绕开双重可变借用。

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Widget};

use crate::render::blit::{self, EdgeColors, HAnchor};
use crate::render::cells::left_eighth;
use crate::render::cells::lower_eighth;
use crate::render::theme::Theme;
use crate::runtime::action::Action;
use crate::runtime::state::AppState;

/// 缩放进度满值(千分比)。到此即完全展开、渲染内容;不足只画外框空壳。
pub(crate) const FULL_SCALE: u16 = 1000;

/// 浮层外框声明:居中尺寸约束 + 是否播放弹出动画。纯静态配置,不含数据/状态。
pub(crate) struct Chrome {
    /// 宽度相对主帧的百分比。
    pub(crate) pct_w: u16,

    /// 高度相对主帧的百分比。
    pub(crate) pct_h: u16,

    /// 宽度绝对下限(字符)。
    pub(crate) min_w: u16,

    /// 高度绝对下限(字符)。
    pub(crate) min_h: u16,

    /// 宽度绝对上限(字符)。
    pub(crate) max_w: u16,

    /// 高度绝对上限(字符)。
    pub(crate) max_h: u16,

    /// 是否播放中心缩放弹出/收起动画;`false` 则瞬时显示。
    pub(crate) animated: bool,

    /// 是否贴非封面侧停靠(抽屉式):`true` 走「贴边 + 仅水平 grow」,停靠侧由当前布局决定
    /// (全屏贴右 / 否则贴左)以避开封面;`false` 居中弹出(对话框)。
    pub(crate) dock: bool,
}

/// 停靠浮层(抽屉式)避开封面的那一侧。
#[derive(Clone, Copy)]
enum Dock {
    /// 贴左(old layout:封面在右栏)。
    Left,

    /// 贴右(全屏:封面在左半)。
    Right,
}

/// 顶栏行数:停靠浮层从其下顶对齐 + 满高,盖住其下面板(old layout 留顶栏;全屏无顶栏,
/// 留同样 1 行保持左右停靠等高)。
const DOCK_TOPBAR: u16 = 1;

/// 浮层对一次按键的响应。
pub(crate) enum OverlayResponse {
    /// 已消费,不再下传。
    Consumed,

    /// 不处理,放行给全局键(半穿透 —— 如 queue 打开时仍可控播放 / 切歌词)。
    Pass,

    /// 产生一个意图,交 App 执行(浮层自身不持有 App)。
    Do(OverlayAction),
}

/// 浮层产生、由 App 执行的意图。
///
/// 浮层私有动作,**不并入** `runtime::action::Action`:以浮层私有光标为参数的意图
/// (如 [`Self::PlayQueueIndex`])没法用「dispatch 时查 `AppState`」的范式表达。
/// 与主 keymap 统一的是 dispatch 入口与动作概念,非枚举合一。
#[derive(Clone, Copy)]
pub(crate) enum OverlayAction {
    /// 退出程序。
    Quit,

    /// 关闭栈顶浮层(触发收起动画)。
    CloseTop,

    /// 播放 queue 中第 `0` 项(下标);App 据此查 [`AppState`] 的队列取歌。
    PlayQueueIndex(usize),
}

/// 浮层抽象:实现方只声明四件事,chrome 自动包办居中 layout + 弹出动画。
///
/// 实现方**不持有动画状态**([`Transition`] 由 stack 托管),也不直接操作 App ——
/// 按键产出 [`OverlayAction`] 回传执行。
///
/// [`Transition`]: crate::render::anim::Transition
pub(crate) trait Overlay {
    /// 外框尺寸约束 + 是否动画。每帧调用,可据自身状态返回不同尺寸。
    fn chrome(&self) -> Chrome;

    /// 构造外框 Block(标题 / 边框色 / 底部提示)。
    ///
    /// # Params:
    ///   - `ctx`: 只读后端态(如队列长度,用于底部 `n / total`)
    ///   - `focused`: 是否持有键盘焦点(栈顶且未在退场),影响边框色
    fn block(&self, ctx: &AppState, theme: &Theme, focused: bool) -> Block<'static>;

    /// 把内容画进 `buf` 的外框内部 `inner`。`inner` 恒为**完全展开**尺寸 —— 动画途中
    /// 内容先按满尺寸渲染到离屏缓冲再按进度搬运可见窗口(不随动画逐帧 reflow),
    /// 实现方不必关心进度。面向 [`Buffer`] 而非 `Frame`,离屏与上屏共用一个入口。
    fn render_content(&self, buf: &mut Buffer, inner: Rect, ctx: &AppState, theme: &Theme);

    /// 处理一个按键,返回 [`OverlayResponse`]。`ctx` 只读后端态(如队列长度,用于
    /// 钳制光标);浮层与 `AppState` 是 App 的平级字段,可同时借用。
    fn on_key(&mut self, key: &KeyEvent, ctx: &AppState) -> OverlayResponse;

    /// 处理一个已查表命中的全局 [`Action`]。返回 `None` 表示本浮层不认这个动作,
    /// 分发器回落到 [`Self::on_key`](裸键路径,浮层私有键)。默认全部不认。
    ///
    /// 与主 keymap 统一的是 dispatch 入口与动作概念(导航族经此跟随键位重映射与
    /// behavior 步长);浮层私有意图仍走 [`OverlayAction`],不并入全局枚举。
    fn on_action(&mut self, _action: Action, _ctx: &AppState) -> Option<OverlayResponse> {
        None
    }
}

/// 统一外框底 Block:圆角边框 + mantle 背景。各 overlay 在此之上加 title / 边框色,
/// 收敛"圆角 + 背景"这段重复样板。
pub(crate) fn base_block(theme: &Theme) -> Block<'static> {
    Block::new()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .style(Style::new().bg(theme.mantle))
}

/// 居中 + 钳制的尺寸计算。
///
/// `pct_w` / `pct_h` 相对 `area` 的百分比,`min_*` / `max_*` 是绝对字符界,最终再被
/// `area` 自身钳制,保证不溢出。
pub(crate) fn centered_rect(
    area: Rect,
    pct_w: u16,
    pct_h: u16,
    min_w: u16,
    min_h: u16,
    max_w: u16,
    max_h: u16,
) -> Rect {
    let w_target = u32::from(area.width) * u32::from(pct_w) / 100;
    let h_target = u32::from(area.height) * u32::from(pct_h) / 100;
    let w = u16::try_from(w_target.clamp(u32::from(min_w), u32::from(max_w))).unwrap_or(min_w);
    let h = u16::try_from(h_target.clamp(u32::from(min_h), u32::from(max_h))).unwrap_or(min_h);
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w, h)
}

/// 渲染一个浮层:居中 → 完全展开画外框 + 内容;动画途中内容按满尺寸离屏渲染,
/// 可见窗口随进度推进(停靠 = 滑入、居中 = 中心揭开),前沿 1/8 块平滑。
///
/// # Params:
///   - `overlay`: 要渲染的浮层
///   - `scale`: 已缓动的缩放进度(千分比),来自 stack 托管的 [`Transition`]
///   - `focused`: 是否持有键盘焦点(影响边框色)
///   - `ctx`: 只读后端态
///
/// [`Transition`]: crate::render::anim::Transition
pub(crate) fn render_overlay<O: Overlay>(
    frame: &mut Frame<'_>,
    area: Rect,
    overlay: &O,
    scale: u16,
    focused: bool,
    ctx: &AppState,
    theme: &Theme,
) {
    let c = overlay.chrome();
    // 停靠浮层:按当前布局选侧(全屏贴右 / 否则贴左),避开封面;否则居中。
    let dock = c.dock.then_some(if ctx.fullscreen.on() {
        Dock::Right
    } else {
        Dock::Left
    });
    let base = match dock {
        Some(d) => dock_rect(area, d, *ctx.cfg.tui().layout().dock_w_pct()),
        None => centered_rect(area, c.pct_w, c.pct_h, c.min_w, c.min_h, c.max_w, c.max_h),
    };
    if scale >= FULL_SCALE {
        // 完全展开:整 cell 外框 + 内容。
        if base.width < 4 || base.height < 3 {
            return;
        }
        frame.render_widget(Clear, base);
        let block = overlay.block(ctx, theme, focused);
        let inner = block.inner(base);
        frame.render_widget(block, base);
        overlay.render_content(frame.buffer_mut(), inner, ctx, theme);
    } else {
        // 动画途中:停靠浮层滑入(内容随前沿平移),居中浮层中心揭开(内容定格)。
        // 离屏渲染在此统一做(动画头几帧被几何 guard 跳过时白渲一次,面积小、可忽略)。
        let off = render_offscreen(base, overlay, focused, ctx, theme);
        match dock {
            Some(d) => draw_dock_slide(frame, base, d, scale, &off, theme),
            None => draw_center_reveal(frame, base, scale, &off, theme),
        }
    }
}

/// 把浮层按完全展开尺寸渲染到与 `full` 等大的离屏缓冲(坐标系与屏幕一致)。
/// 动画途中每帧重渲一次 —— 区域小、动画短,代价与 sidebar sweep 同量级。
fn render_offscreen<O: Overlay>(
    full: Rect,
    overlay: &O,
    focused: bool,
    ctx: &AppState,
    theme: &Theme,
) -> Buffer {
    let mut buf = Buffer::empty(full);
    let block = overlay.block(ctx, theme, focused);
    let inner = block.inner(full);
    block.render(full, &mut buf);
    overlay.render_content(&mut buf, inner, ctx, theme);
    buf
}

/// 计算停靠浮层「完全展开」矩形:左右**同宽(配置 `tui.layout.dock_w_pct`)同高**
/// (从顶栏下顶对齐 + 满高),只在贴边侧不同 —— old layout 贴左、全屏贴右,
/// 均避开另一侧的封面。
fn dock_rect(area: Rect, dock: Dock, dock_w_pct: u16) -> Rect {
    let w = pct_of(area.width, dock_w_pct);
    let top = DOCK_TOPBAR.min(area.height);
    let h = area.height.saturating_sub(top);
    let x = match dock {
        Dock::Left => area.x,
        Dock::Right => area.right().saturating_sub(w),
    };
    Rect::new(x, area.y + top, w, h)
}

/// 按百分比取尺寸,钳到 `[0, total]`(不碰 `as` 强转)。
fn pct_of(total: u16, p: u16) -> u16 {
    u16::try_from(u32::from(total) * u32::from(p) / 100)
        .unwrap_or(total)
        .min(total)
}

/// 停靠浮层的进退场动画:满高不变,真实面板(`off` = 满尺寸离屏渲染)沿水平方向从
/// 停靠边缘滑入 / 滑出 —— 内容随前沿整格平移、前沿侧的边框最先进场,前沿分数格用
/// 1/8 八分块平滑(体色 mantle)。长满后由 [`render_overlay`] 切回直绘路径。
fn draw_dock_slide(
    frame: &mut Frame<'_>,
    full: Rect,
    dock: Dock,
    scale: u16,
    off: &Buffer,
    theme: &Theme,
) {
    if full.width == 0 || full.height == 0 {
        return;
    }
    let cur_w_e = u32::from(full.width) * 8 * u32::from(scale) / u32::from(FULL_SCALE);
    // 太窄画不出有意义面板,跳过这一帧。
    if cur_w_e < 8 {
        return;
    }
    let anchor = match dock {
        Dock::Left => HAnchor::Left,
        Dock::Right => HAnchor::Right,
    };
    let edge = EdgeColors {
        fill: theme.mantle,
        bg: theme.base,
    };
    blit::slide_h(frame.buffer_mut(), off, full, cur_w_e, anchor, edge);
}

/// 居中浮层的进退场动画:真实面板(`off` = 满尺寸离屏渲染)以 `base` 中心为锚,按
/// `scale` 拉开一个窗口**原位揭出**内容(reveal,内容定格终位),窗口宽高都到 1/8 cell
/// 精度 —— 完全覆盖的整格直接搬运离屏内容,四沿分数格画体色八分块补亚格平滑。
///
/// 终端 block 字符只在「底对齐」(下八分块)/「左对齐」(左八分块)两个方向有完整 8 档,
/// 顶沿 / 右沿用 `fg`/`bg` 反色补齐;四角的双轴分数格按垂直近似(水平略溢出 ≤ 7/8,
/// 缩放途中肉眼基本无感)。到 `scale >= FULL_SCALE` 时由 [`render_overlay`] 切回直绘。
fn draw_center_reveal(frame: &mut Frame<'_>, base: Rect, scale: u16, off: &Buffer, theme: &Theme) {
    // 1/8 cell 单位下的当前尺寸(中心缩放)。
    let full_w_e = u32::from(base.width) * 8;
    let full_h_e = u32::from(base.height) * 8;
    let cur_w_e = full_w_e * u32::from(scale) / u32::from(FULL_SCALE);
    let cur_h_e = full_h_e * u32::from(scale) / u32::from(FULL_SCALE);
    // 太小画不出有意义的面板,跳过这一帧。
    if cur_w_e < 4 * 8 || cur_h_e < 3 * 8 {
        return;
    }

    // 面板边界(1/8 坐标系,原点为 area 左上)。
    let cx_e = u32::from(base.x) * 8 + full_w_e / 2;
    let cy_e = u32::from(base.y) * 8 + full_h_e / 2;
    let left_e = cx_e.saturating_sub(cur_w_e / 2);
    let right_e = left_e + cur_w_e;
    let top_e = cy_e.saturating_sub(cur_h_e / 2);
    let bottom_e = top_e + cur_h_e;

    let col0 = left_e / 8;
    let col1 = right_e.div_ceil(8);
    let row0 = top_e / 8;
    let row1 = bottom_e.div_ceil(8);

    // 先 Clear 整 cell 包围盒,防主 UI 从面板边缘格透出。
    let outer = Rect::new(
        u16::try_from(col0).unwrap_or(base.x),
        u16::try_from(row0).unwrap_or(base.y),
        u16::try_from(col1.saturating_sub(col0)).unwrap_or(0),
        u16::try_from(row1.saturating_sub(row0)).unwrap_or(0),
    );
    frame.render_widget(Clear, outer);

    // 完全覆盖的整格窗口:离屏内容原位揭出(内容定格,边框在窗口长到满前不可见)。
    let in_c0 = u16::try_from(left_e.div_ceil(8)).unwrap_or(base.x);
    let in_c1 = u16::try_from(right_e / 8).unwrap_or(base.x);
    let in_r0 = u16::try_from(top_e.div_ceil(8)).unwrap_or(base.y);
    let in_r1 = u16::try_from(bottom_e / 8).unwrap_or(base.y);
    let buf = frame.buffer_mut();
    if in_c1 > in_c0 && in_r1 > in_r0 {
        blit::copy_window(
            buf,
            off,
            Rect::new(in_c0, in_r0, in_c1 - in_c0, in_r1 - in_r0),
            in_c0,
            in_r0,
        );
    }

    // 四沿分数格:体色八分块补亚格平滑。
    let fill = theme.mantle;
    let bg = theme.base;
    for col in col0..col1 {
        let c_lo = col * 8;
        let c_hi = c_lo + 8;
        let hcov = right_e.min(c_hi).saturating_sub(left_e.max(c_lo));
        if hcov == 0 {
            continue;
        }
        // 面板右沿落在本列(或更左)→ 覆盖 cell 左部(左对齐);否则是左沿列,覆盖右部(反色)。
        let paint_left_part = right_e <= c_hi;
        for row in row0..row1 {
            let r_lo = row * 8;
            let r_hi = r_lo + 8;
            let vcov = bottom_e.min(r_hi).saturating_sub(top_e.max(r_lo));
            if vcov == 0 || (hcov >= 8 && vcov >= 8) {
                // 整格已被离屏内容覆盖。
                continue;
            }
            // 面板向下延伸过本 cell → 覆盖下部(底对齐,上沿);否则覆盖上部(顶对齐,下沿,反色)。
            let bottom_aligned = bottom_e >= r_hi;
            let (glyph, style) = edge_cell(hcov, vcov, paint_left_part, bottom_aligned, fill, bg);
            let (Ok(x), Ok(y)) = (u16::try_from(col), u16::try_from(row)) else {
                continue;
            };
            buf.set_string(x, y, glyph, style);
        }
    }
}

/// 选一个**边缘**分数格的块字符 + 样式。`hcov` / `vcov` 是该 cell 被面板覆盖的
/// 1/8 单位数(至少一轴 `< 8`;双满格走离屏搬运,不进这里)。
///
/// 纯垂直 / 水平边用对应方向八分块(顶 / 右沿走 `fg`/`bg` 反色补齐);
/// 双轴分数的角格按垂直近似。
fn edge_cell(
    hcov: u32,
    vcov: u32,
    paint_left_part: bool,
    bottom_aligned: bool,
    fill: Color,
    bg: Color,
) -> (&'static str, Style) {
    if hcov >= 8 {
        // 纯垂直边(上 / 下沿)。
        vertical_eighth(vcov, bottom_aligned, fill, bg)
    } else if vcov >= 8 {
        // 纯水平边(左 / 右沿)。
        if paint_left_part {
            (left_eighth(hcov), Style::new().fg(fill))
        } else {
            (left_eighth(8 - hcov), Style::new().fg(bg).bg(fill))
        }
    } else {
        // 角:双轴分数,单字符表达不了,按垂直近似(水平略溢出)。
        vertical_eighth(vcov, bottom_aligned, fill, bg)
    }
}

/// 垂直方向 `vcov/8` 实心:底对齐用下八分块;顶对齐用 `fg`/`bg` 反色补齐。
fn vertical_eighth(
    vcov: u32,
    bottom_aligned: bool,
    fill: Color,
    bg: Color,
) -> (&'static str, Style) {
    if bottom_aligned {
        (lower_eighth(vcov), Style::new().fg(fill))
    } else {
        (lower_eighth(8 - vcov), Style::new().fg(bg).bg(fill))
    }
}

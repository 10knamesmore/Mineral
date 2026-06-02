//! 浮层基础组件:统一的 [`Overlay`] 抽象。
//!
//! chrome 自动提供居中 layout + 中心缩放弹出动画;实现方只声明四件事(外框尺寸、
//! 外框 Block、内容渲染、按键响应),既不持有动画状态(由 stack 托管 [`Transition`]),
//! 也不直接操作 App —— 按键产出 [`OverlayAction`] 回传执行,绕开双重可变借用。

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, BorderType, Borders, Clear};

use crate::render::cells::{left_eighth, lower_eighth};
use crate::render::theme::Theme;
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

/// 左停靠(old layout,封面在右栏)宽度百分比。
const DOCK_LEFT_PCT: u16 = 36;

/// 右停靠(全屏,封面在左)宽度百分比。
const DOCK_RIGHT_PCT: u16 = 46;

/// 全屏右停靠的高度百分比(垂直居中)。
const DOCK_FS_H_PCT: u16 = 90;

/// old layout 顶栏行数:左停靠浮层从其下顶对齐,盖住其下的 playlist / lyrics,只留顶栏。
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

    /// 把内容画进外框内部 `inner`。仅完全展开(`scale >= FULL_SCALE`)时调用;
    /// 动画途中只画空壳,避免窄尺寸下内容 reflow 抖动。
    fn render_content(&self, frame: &mut Frame<'_>, inner: Rect, ctx: &AppState, theme: &Theme);

    /// 处理一个按键,返回 [`OverlayResponse`]。`ctx` 只读后端态(如队列长度,用于
    /// 钳制光标);浮层与 `AppState` 是 App 的平级字段,可同时借用。
    fn on_key(&mut self, key: &KeyEvent, ctx: &AppState) -> OverlayResponse;
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

/// 渲染一个浮层:居中 → 完全展开画外框 + 内容;动画途中画 1/8 块平滑色块。
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
    let dock = c.dock.then_some(if ctx.fullscreen {
        Dock::Right
    } else {
        Dock::Left
    });
    let base = match dock {
        Some(d) => dock_rect(area, d),
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
        overlay.render_content(frame, inner, ctx, theme);
    } else {
        // 动画途中(无边框 / 无内容,避免 reflow):停靠浮层走「贴边 + 仅水平 grow」,
        // 居中浮层走「中心双轴缩放」。
        match dock {
            Some(d) => draw_h_grow_shell(frame, base, d, scale, theme),
            None => draw_shell(frame, base, scale, theme),
        }
    }
}

/// 计算停靠浮层「完全展开」矩形,按侧分别布局(避开另一侧的封面):
///   - 左停靠(old layout):贴左缘、从顶栏下**顶对齐 + 满高**,盖住其下 playlist / lyrics,只留顶栏;
///   - 右停靠(全屏):贴右缘、垂直居中、占 [`DOCK_FS_H_PCT`] 高(较高)。
fn dock_rect(area: Rect, dock: Dock) -> Rect {
    match dock {
        Dock::Left => {
            let w = pct_of(area.width, DOCK_LEFT_PCT);
            let top = DOCK_TOPBAR.min(area.height);
            Rect::new(area.x, area.y + top, w, area.height.saturating_sub(top))
        }
        Dock::Right => {
            let w = pct_of(area.width, DOCK_RIGHT_PCT);
            let h = pct_of(area.height, DOCK_FS_H_PCT);
            let y = area.y + area.height.saturating_sub(h) / 2;
            Rect::new(area.right().saturating_sub(w), y, w, h)
        }
    }
}

/// 按百分比取尺寸,钳到 `[0, total]`(不碰 `as` 强转)。
fn pct_of(total: u16, p: u16) -> u16 {
    u16::try_from(u32::from(total) * u32::from(p) / 100)
        .unwrap_or(total)
        .min(total)
}

/// 停靠浮层的进退场动画:满高不变,只在水平方向从停靠边缘按 `scale`(千分比)长出 / 收回,
/// 生长边用 1/8 cell 八分块平滑(对齐 [`draw_shell`] 的精度)。长满后由 [`render_overlay`]
/// 切成带边框面板 + 内容。
fn draw_h_grow_shell(frame: &mut Frame<'_>, full: Rect, dock: Dock, scale: u16, theme: &Theme) {
    if full.width == 0 || full.height == 0 {
        return;
    }
    let full_w_e = u32::from(full.width) * 8;
    let cur_w_e = full_w_e * u32::from(scale) / u32::from(FULL_SCALE);
    // 太窄画不出有意义面板,跳过这一帧。
    if cur_w_e < 8 {
        return;
    }

    // 1/8 坐标系:停靠边缘固定,生长边按 cur_w_e 推进。
    let left_edge_e = u32::from(full.x) * 8;
    let right_edge_e = u32::from(full.right()) * 8;
    let (left_e, right_e) = match dock {
        Dock::Left => (left_edge_e, left_edge_e + cur_w_e),
        Dock::Right => (right_edge_e.saturating_sub(cur_w_e), right_edge_e),
    };
    let col0 = left_e / 8;
    let col1 = right_e.div_ceil(8);

    // 先 Clear 包围盒(满高),防底层 UI 从边缘格透出。
    let outer = Rect::new(
        u16::try_from(col0).unwrap_or(full.x),
        full.y,
        u16::try_from(col1.saturating_sub(col0)).unwrap_or(0),
        full.height,
    );
    frame.render_widget(Clear, outer);

    // 体色随 scale 从骨架色 surface1 渐变到完成态面板体色 mantle,切带边框面板时体色连续。
    let fill = crate::render::color::lerp_color(
        theme.surface1,
        theme.mantle,
        u64::from(scale),
        u64::from(FULL_SCALE),
    );
    let bg = theme.base;
    let y1 = full.y.saturating_add(full.height);
    let buf = frame.buffer_mut();
    for col in col0..col1 {
        let c_lo = col * 8;
        let c_hi = c_lo + 8;
        let hcov = right_e.min(c_hi).saturating_sub(left_e.max(c_lo));
        if hcov == 0 {
            continue;
        }
        let (glyph, style) = if hcov >= 8 {
            ("█", Style::new().fg(fill))
        } else {
            // 分数生长边:左停靠生长边在右(左对齐填充);右停靠生长边在左(反色右对齐填充)。
            match dock {
                Dock::Left => (left_eighth(hcov), Style::new().fg(fill)),
                Dock::Right => (left_eighth(8 - hcov), Style::new().fg(bg).bg(fill)),
            }
        };
        let Ok(x) = u16::try_from(col) else {
            continue;
        };
        for y in full.y..y1 {
            buf.set_string(x, y, glyph, style);
        }
    }
}

/// 动画途中的平滑"空壳":以 `base` 中心为锚,按 `scale` 缩放出一个色块,**宽高都到
/// 1/8 cell 精度**,消除整格缩放的台阶感。
///
/// 终端 block 字符只在「底对齐」(下八分块)/「左对齐」(左八分块)两个方向有完整 8 档,
/// 顶沿 / 右沿用 `fg`/`bg` 反色补齐;四角的双轴分数格按垂直近似(水平略溢出 ≤ 7/8,
/// 缩放途中肉眼基本无感)。到 `scale >= FULL_SCALE` 时由 [`render_overlay`] 切回带边框
/// 的整 cell 面板 + 内容。
fn draw_shell(frame: &mut Frame<'_>, base: Rect, scale: u16, theme: &Theme) {
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

    // 色块体色随 scale 从骨架色 surface1 渐变到完成态面板体色 mantle:到位(scale→FULL)
    // 时已等于 base_block 的背景,切成带边框面板时体色连续,只剩边框 / 内容淡入,不突变。
    let fill = crate::render::color::lerp_color(
        theme.surface1,
        theme.mantle,
        u64::from(scale),
        u64::from(FULL_SCALE),
    );
    let bg = theme.base;
    let buf = frame.buffer_mut();
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
            if vcov == 0 {
                continue;
            }
            // 面板向下延伸过本 cell → 覆盖下部(底对齐,上沿);否则覆盖上部(顶对齐,下沿,反色)。
            let bottom_aligned = bottom_e >= r_hi;
            let (glyph, style) = shell_cell(hcov, vcov, paint_left_part, bottom_aligned, fill, bg);
            let (Ok(x), Ok(y)) = (u16::try_from(col), u16::try_from(row)) else {
                continue;
            };
            buf.set_string(x, y, glyph, style);
        }
    }
}

/// 选一格的块字符 + 样式。`hcov` / `vcov` 是该 cell 被面板覆盖的 1/8 单位数(`1..=8`)。
///
/// 内部格 `█`;纯垂直 / 水平边用对应方向八分块(顶 / 右沿走 `fg`/`bg` 反色补齐);
/// 双轴分数的角格按垂直近似。
fn shell_cell(
    hcov: u32,
    vcov: u32,
    paint_left_part: bool,
    bottom_aligned: bool,
    fill: Color,
    bg: Color,
) -> (&'static str, Style) {
    let h_full = hcov >= 8;
    let v_full = vcov >= 8;
    if h_full && v_full {
        ("█", Style::new().fg(fill))
    } else if h_full {
        // 纯垂直边(上 / 下沿)。
        vertical_eighth(vcov, bottom_aligned, fill, bg)
    } else if v_full {
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

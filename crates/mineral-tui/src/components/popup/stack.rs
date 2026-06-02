//! 浮层栈:统一托管所有浮层的弹出动画与生命周期,自底向上渲染,并把按键路由到
//! 活跃栈顶。浮层的 [`Transition`] 由这里持有 —— 实现方只声明 `animated`,推进 /
//! 进退场 / 延迟移除全在本模块,使用方碰都不碰动画。

use crossterm::event::KeyEvent;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::widgets::Block;

use crate::components::popup::component::{Chrome, Overlay, OverlayResponse, render_overlay};
use crate::components::popup::confirm::ConfirmOverlay;
use crate::components::popup::disconnect::DisconnectOverlay;
use crate::components::popup::queue::QueueOverlay;
use crate::render::anim::Transition;
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 弹出 / 收起动画时长(tick 数)。主循环 60fps、每 tick 推进一步,18 tick ≈ 300ms。
const ANIM_TICKS: u16 = 18;

/// 不动画时用的"一帧到位"时长。
const INSTANT_TICKS: u16 = 1;

/// 一种具体浮层。闭集 enum + 手动转发 trait 方法(强类型、无 `dyn`、契合内部结构化)。
pub(crate) enum OverlayKind {
    /// 浮动播放队列。
    Queue(QueueOverlay),

    /// 退出确认。
    Confirm(ConfirmOverlay),

    /// daemon 断连提示。
    Disconnect(DisconnectOverlay),
}

impl OverlayKind {
    /// 浮动队列,光标定位到 `sel`(通常是在播歌下标)。
    pub(crate) fn queue(sel: usize) -> Self {
        Self::Queue(QueueOverlay::new(sel))
    }

    /// 退出确认。
    pub(crate) fn confirm() -> Self {
        Self::Confirm(ConfirmOverlay)
    }

    /// daemon 断连提示。
    pub(crate) fn disconnect() -> Self {
        Self::Disconnect(DisconnectOverlay)
    }
}

impl Overlay for OverlayKind {
    fn chrome(&self) -> Chrome {
        match self {
            Self::Queue(o) => o.chrome(),
            Self::Confirm(o) => o.chrome(),
            Self::Disconnect(o) => o.chrome(),
        }
    }

    fn block(&self, ctx: &AppState, theme: &Theme, focused: bool) -> Block<'static> {
        match self {
            Self::Queue(o) => o.block(ctx, theme, focused),
            Self::Confirm(o) => o.block(ctx, theme, focused),
            Self::Disconnect(o) => o.block(ctx, theme, focused),
        }
    }

    fn render_content(&self, frame: &mut Frame<'_>, inner: Rect, ctx: &AppState, theme: &Theme) {
        match self {
            Self::Queue(o) => o.render_content(frame, inner, ctx, theme),
            Self::Confirm(o) => o.render_content(frame, inner, ctx, theme),
            Self::Disconnect(o) => o.render_content(frame, inner, ctx, theme),
        }
    }

    fn on_key(&mut self, key: &KeyEvent, ctx: &AppState) -> OverlayResponse {
        match self {
            Self::Queue(o) => o.on_key(key, ctx),
            Self::Confirm(o) => o.on_key(key, ctx),
            Self::Disconnect(o) => o.on_key(key, ctx),
        }
    }
}

/// 一个挂载在栈上的浮层:具体浮层 + 框架托管的动画进度。
struct Mounted {
    /// 具体浮层。
    kind: OverlayKind,

    /// 弹出 / 收起动画进度(纯 UI-local,逐 tick 推进,不被 server snapshot 覆盖)。
    anim: Transition,
}

impl Mounted {
    /// 挂载一个浮层并立即启动进场:按 `chrome().animated` 决定播动画还是一帧到位。
    fn new(kind: OverlayKind) -> Self {
        let ticks = if kind.chrome().animated {
            ANIM_TICKS
        } else {
            INSTANT_TICKS
        };
        let mut anim = Transition::new(ticks);
        anim.enter();
        Self { kind, anim }
    }
}

/// 浮层栈。空栈表示无浮层,按键走主视图。
pub(crate) struct OverlayStack {
    /// 自底向上的浮层(末尾 = 最上层)。
    stack: Vec<Mounted>,
}

impl OverlayStack {
    /// 新建空栈。
    pub(crate) fn new() -> Self {
        Self { stack: Vec::new() }
    }

    /// 压入一个浮层并启动进场动画。
    pub(crate) fn push(&mut self, kind: OverlayKind) {
        self.stack.push(Mounted::new(kind));
    }

    /// 关闭栈顶浮层:触发收起动画,延迟到归零后由 [`Self::tick`] 真正移除。
    pub(crate) fn close_top(&mut self) {
        if let Some(m) = self.stack.last_mut() {
            m.anim.leave();
        }
    }

    /// 推进栈内所有浮层的动画一拍,并清理已收尾(归零)的退场项。
    pub(crate) fn tick(&mut self) {
        for m in &mut self.stack {
            m.anim.tick();
        }
        self.stack.retain(|m| m.anim.active());
    }

    /// 把按键路由到活跃栈顶(最上面一个未在退场的浮层)。
    ///
    /// # Return:
    ///   `None` = 无活跃浮层,按键应走主视图;`Some(resp)` = 活跃栈顶的响应。
    pub(crate) fn on_key(&mut self, key: &KeyEvent, ctx: &AppState) -> Option<OverlayResponse> {
        self.active_top_mut().map(|m| m.kind.on_key(key, ctx))
    }

    /// 自底向上渲染所有浮层;活跃栈顶标记为 `focused`(影响边框色)。
    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect, ctx: &AppState, theme: &Theme) {
        let top = self.active_top_index();
        for (i, m) in self.stack.iter().enumerate() {
            let focused = Some(i) == top;
            render_overlay(frame, area, &m.kind, m.anim.eased(), focused, ctx, theme);
        }
    }

    /// 当前栈内浮层数(含正在退场、尚未被 [`Self::tick`] 移除的)。
    pub(crate) fn len(&self) -> usize {
        self.stack.len()
    }

    /// 栈内是否有断连提示(据此进入 fatal 模式:跳过后端同步、任意键退出)。
    pub(crate) fn is_disconnected(&self) -> bool {
        self.stack
            .iter()
            .any(|m| matches!(m.kind, OverlayKind::Disconnect(_)))
    }

    /// 把栈内 queue 浮层的光标钳到 `[0, len-1]`(队列变短后防越界)。
    pub(crate) fn clamp_queue(&mut self, len: usize) {
        for m in &mut self.stack {
            if let OverlayKind::Queue(q) = &mut m.kind {
                q.clamp(len);
            }
        }
    }

    /// 活跃栈顶(最上面一个未在退场的浮层)的下标。
    fn active_top_index(&self) -> Option<usize> {
        self.stack.iter().rposition(|m| !m.anim.leaving())
    }

    /// 活跃栈顶的可变引用。
    fn active_top_mut(&mut self) -> Option<&mut Mounted> {
        self.stack.iter_mut().rev().find(|m| !m.anim.leaving())
    }

    /// 测试用:栈内 queue 浮层的光标下标(无 queue 时 `None`)。
    #[cfg(test)]
    pub(crate) fn queue_sel(&self) -> Option<usize> {
        self.stack.iter().find_map(|m| match &m.kind {
            OverlayKind::Queue(q) => Some(q.cursor()),
            _ => None,
        })
    }
}

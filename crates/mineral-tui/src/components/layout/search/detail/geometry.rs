//! 详情面板的头部、封面与列表区域几何，供绘制、封面飞行端点和行级菜单定位共用。

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::widgets::{Block, Borders};

/// detail 面板头图区：与稳态帧绘制共用外框内区 → [`split_frame`] → [`split_head`] 的几何，
/// page morph 封面飞行层据此取 search 端点。面板画不下（内区过小 / 窄到砍掉 cover 栏）
/// 为 `None`。
///
/// # Params:
///   - `area`: detail 面板整区（含边框）
///   - `is_artist`: 栈顶帧是否 artist（头部三栏布局）
pub(crate) fn header_cover_area(area: Rect, is_artist: bool) -> Option<Rect> {
    let inner = Block::new().borders(Borders::ALL).inner(area);
    if inner.height < 2 || inner.width == 0 {
        return None;
    }
    let (head, _delim, _body) = split_frame(inner);
    let (cover_a, _meta_a, _right_a) = split_head(head, is_artist);
    (cover_a.width > 0).then_some(cover_a)
}

/// 纵分三段:头部(~45%,左右两张正方 cover) + 1 行 meta↔list 分隔 + 列表 body。
/// 夹下限保矮面板仍有基本头部、夹上限给分隔 + 列表留可视行。
pub(super) fn split_frame(inner: Rect) -> (Rect, Rect, Rect) {
    let head_h = (inner.height * 9 / 20)
        .max(6)
        .min(inner.height.saturating_sub(4));
    let [head, delim, body] = Layout::vertical([
        Constraint::Length(head_h),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    (head, delim, body)
}

/// artist 帧主体再分：顶 1 行双区 Tab + 其下列表区。抽出供渲染与 [`detail_list_area`] 共用，
/// 锚点与渲染走同一处几何。
pub(super) fn split_artist_body(body: Rect) -> (Rect, Rect) {
    let [tabs, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(body);
    (tabs, list)
}

/// 当前栈顶帧「列表区」相对面板内区 `inner` 的屏幕矩形：去头部，artist 帧再去 Tab 行。
/// 渲染端与行级菜单锚点共用此一处几何——菜单贴的行恒等于渲染出的行。该列表区**无**自己
/// 的 block 边框（边框归整个 detail 面板），故其视口数学（区高 − 表头一行）与 browse 带
/// 边框面板不同，锚点侧据此还原行 y。
///
/// # Params:
///   - `inner`: detail 面板去外框后的内区
///   - `is_artist`: 是否 artist 帧（多减一行 Tab）
pub(crate) fn detail_list_area(inner: Rect, is_artist: bool) -> Rect {
    let (_head, _delim, body) = split_frame(inner);
    if is_artist {
        split_artist_body(body).1
    } else {
        body
    }
}

/// 头部横分：左正方头图 + 中元数据；`with_right` 时右侧再切一栏放选中项 cover（artist 帧）。
///
/// 窄面板响应式降级：宽度容不下三栏退两栏、容不下两栏退纯 meta（左 cover 栏宽置 0，由调用方
/// 按 `width == 0` 跳过绘制）。
pub(super) fn split_head(head: Rect, with_right: bool) -> (Rect, Rect, Option<Rect>) {
    // 单张 cover 栏的最小可视宽度断点：窄于此画出来只是无意义细条，宁可砍掉让位 meta。
    const MIN_COVER_W: u16 = 8;
    if with_right && head.width >= MIN_COVER_W * 3 {
        let [cover_a, meta_a, right_a] = Layout::horizontal([
            Constraint::Percentage(28),
            Constraint::Min(1),
            Constraint::Percentage(28),
        ])
        .areas(head);
        (cover_a, meta_a, Some(right_a))
    } else if head.width >= MIN_COVER_W * 2 {
        let [cover_a, meta_a] =
            Layout::horizontal([Constraint::Percentage(30), Constraint::Min(1)]).areas(head);
        (cover_a, meta_a, None)
    } else {
        (Rect::new(head.x, head.y, 0, head.height), head, None)
    }
}

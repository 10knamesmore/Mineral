//! 紧凑分类窗在列表左侧展开，焦点边框在小窗与列表之间滑动，只在确认后切换内容。

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::Style;
use ratatui::text::Line;
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Row, StatefulWidget, Table, TableState, Widget,
};

use super::CollectionList;
use crate::components::layout::shared::transform::lerp_rect;
use crate::render::anim::{Toggle, lerp_u16};
use crate::render::theme::Theme;
use crate::runtime::action::SelectionMove;
use crate::runtime::collection::CollectionGroup;
use crate::runtime::state::LibraryData;

/// 预选类型和窗口动画共存，取消时不改动列表。
pub(super) struct TypeDrawer {
    /// 展开目标同时决定是否接收导航按键。
    pub(super) reveal: Toggle,

    /// 仅在确认时提交的类型，取消不影响内容列表。
    pub(super) selected: CollectionGroup,
}

impl TypeDrawer {
    /// 从关闭状态开始，时长由浏览页传入。
    pub(super) fn new(ticks: u16) -> Self {
        Self {
            reveal: Toggle::new(ticks),
            selected: CollectionGroup::Playlists,
        }
    }

    /// 根时钟推进分类窗的展开与收起。
    pub(super) fn tick(&mut self) {
        self.reveal.tick();
    }

    /// 配置热更保留窗口当前位置。
    pub(super) fn retempo(&mut self, ticks: u16) {
        self.reveal.retempo(ticks);
    }

    /// 抽屉与列表共用这份几何，图片和菜单也据此定位。
    pub(super) fn areas(&self, area: Rect) -> (Rect, Rect) {
        let width = lerp_u16(0, 20.min(area.width / 2), self.reveal.eased_in_out());
        (
            Rect::new(area.x, area.y, width.saturating_sub(1), area.height.min(4)),
            Rect::new(area.x + width, area.y, area.width - width, area.height),
        )
    }

    /// 在两侧内容之后叠画焦点边框，与抽屉共用进度以保持反向切换连续。
    pub(super) fn paint_focus_ring(&self, area: Rect, theme: &Theme, buf: &mut Buffer) {
        if self.reveal.settled() {
            return;
        }
        let (drawer, content) = self.areas(area);
        let ring = lerp_rect(content, drawer, self.reveal.eased_in_out());
        Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(theme.accent))
            .render(ring, buf);
    }

    /// 两个选项共用一层外框；滑动期间压暗固定边框，高亮交给焦点层。
    pub(super) fn paint(&self, area: Rect, library: &LibraryData, theme: &Theme, buf: &mut Buffer) {
        let (area, _) = self.areas(area);
        if area.is_empty() {
            return;
        }
        let border = if self.reveal.on() && self.reveal.settled() {
            theme.accent
        } else {
            theme.surface1
        };
        let rows = [
            (CollectionGroup::Playlists, library.playlists.len()),
            (CollectionGroup::Albums, library.albums.len()),
        ]
        .map(|(group, count)| {
            Row::new([
                Cell::from(group.label()),
                Cell::from(Line::from(count.to_string()).right_aligned())
                    .style(Style::new().fg(theme.overlay)),
            ])
        });
        let table = Table::new(rows, [Constraint::Min(0), Constraint::Length(3)])
            .block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::new().fg(border))
                    .title(Line::from(" collections ").style(Style::new().fg(border))),
            )
            .style(Style::new().fg(theme.subtext))
            .highlight_symbol("▌ ")
            .row_highlight_style(
                Style::new()
                    .fg(if self.reveal.on() {
                        theme.accent
                    } else {
                        theme.subtext
                    })
                    .bg(theme.surface0),
            );
        let selected = match self.selected {
            CollectionGroup::Playlists => 0,
            CollectionGroup::Albums => 1,
        };
        StatefulWidget::render(
            table,
            area,
            buf,
            &mut TableState::default().with_selected(Some(selected)),
        );
    }
}

impl CollectionList {
    /// 当前焦点是否在类型窗。
    pub(crate) fn choosing_type(&self) -> bool {
        self.drawer.reveal.on()
    }

    /// 窗口移动时冻结列表和图片布局目标。
    pub(crate) fn drawer_settled(&self) -> bool {
        self.drawer.reveal.settled()
    }

    /// 在当前类型上打开窗口，丢弃上一次取消的预选。
    pub(crate) fn open_types(&mut self) {
        self.drawer.selected = self.group;
        self.drawer.reveal.set(true);
        mineral_log::debug!(target: "tui", group = self.group.label(), "open collections navigation");
    }

    /// 取消选择，列表和查询保持原状。
    pub(crate) fn close_types(&mut self) {
        self.drawer.reveal.set(false);
        mineral_log::debug!(target: "tui", group = self.group.label(), "close collections navigation");
    }

    /// 在两种类型间预选；不触碰任何列表位置。
    pub(crate) fn move_type(&mut self, movement: SelectionMove) {
        self.drawer.selected = match movement {
            SelectionMove::Up(_) | SelectionMove::First => CollectionGroup::Playlists,
            SelectionMove::Down(_) | SelectionMove::Last => CollectionGroup::Albums,
        };
    }

    /// 提交预选并收回窗口，恢复该类型此前保留的光标与查询。
    pub(crate) fn confirm_type(&mut self) {
        let previous = self.group;
        self.select_group(self.drawer.selected);
        self.drawer.reveal.set(false);
        mineral_log::debug!(target: "tui", previous = previous.label(), group = self.group.label(), "confirm collection type");
    }

    /// 当前帧给列表、缩略图和菜单使用的实际区域。
    pub(crate) fn content_area(&self, area: Rect) -> Rect {
        self.drawer.areas(area).1
    }
}

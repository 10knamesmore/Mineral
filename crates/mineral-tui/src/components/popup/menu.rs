//! PopMenu:锚定弹出的轻量选择菜单,一个组件多处复用(上下文操作菜单 / 复制菜单,
//! 后续还有 `@` 源补全 / `$` 类型单选 / 加入歌单选择器)。
//!
//! 定位走 [`Placement`](super::placement::Placement) 锚定算法(非居中/dock),
//! 进退场是方向性揭开(贴锚边先出现),动画由
//! [`OverlayStack`](super::stack::OverlayStack) 托管。

use crossterm::event::{KeyCode, KeyEvent};
use mineral_model::Song;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::components::popup::component::{
    Chrome, Overlay, OverlayAction, OverlayResponse, base_block,
};
use crate::components::popup::placement::Placement;
use crate::render::theme::Theme;
use crate::runtime::action::{Action, SelectionMove};
use crate::runtime::state::AppState;

/// 菜单确认后产出、由 App 执行的动作。
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum MenuAction {
    /// 插播:插到当前曲之后(下一首播)。
    PlayNext(Box<Song>),

    /// 追加到队列末尾。
    Append(Box<Song>),

    /// 下载这首。
    Download(Box<Song>),

    /// 把文本写进系统剪贴板(复制菜单;文本在构造菜单时就渲染好)。
    Copy(String),

    /// 自定义复制模板:确认时把实体发给 daemon 脚本运行时渲染,文本回来再进
    /// 剪贴板(函数在 daemon 的 VM 里,client 侧只有下标与数据)。
    CopyTemplate {
        /// 模板下标(0-based,对位 config `copy.templates` 数组序)。
        index: usize,

        /// 模板作用的实体(构造菜单时捕获)。
        ctx: mineral_protocol::CopyTemplateCtx,
    },
}

/// 一个菜单项。
pub(crate) struct MenuItem {
    /// 快捷字母:按下直达并执行。`None` = 仅导航 + 确认可达。
    /// 避开 `j`/`k`/`h`/`q`/`l`(见模块文档)。
    pub(crate) hotkey: Option<char>,

    /// 显示标签。
    pub(crate) label: String,

    /// 确认后产出的动作。
    pub(crate) action: MenuAction,

    /// 危险项:红色样式(置底由构造方排序保证)。
    pub(crate) destructive: bool,
}

impl MenuItem {
    /// 带快捷字母的普通项(非危险)。
    pub(crate) fn keyed(hotkey: char, label: impl Into<String>, action: MenuAction) -> Self {
        Self {
            hotkey: Some(hotkey),
            label: label.into(),
            action,
            destructive: false,
        }
    }
}

/// 锚定弹出菜单。
pub(crate) struct PopMenu {
    /// 标题。
    title: String,

    /// 全部菜单项。
    items: Vec<MenuItem>,

    /// 光标下标。
    sel: usize,

    /// 锚点矩形(屏幕坐标)。
    anchor: Rect,

    /// 首选弹出方向。
    placement: Placement,
}

impl PopMenu {
    /// 新建菜单,光标在首项。
    pub(crate) fn new(
        title: impl Into<String>,
        items: Vec<MenuItem>,
        anchor: Rect,
        placement: Placement,
    ) -> Self {
        Self {
            title: title.into(),
            items,
            sel: 0,
            anchor,
            placement,
        }
    }

    /// 确认当前选中项。
    fn confirm(&self) -> OverlayResponse {
        match self.items.get(self.sel) {
            Some(it) => OverlayResponse::Do(OverlayAction::Menu(it.action.clone())),
            None => OverlayResponse::Consumed,
        }
    }

    /// 期望内容宽度:标题与最宽项取大,含快捷字母列与左右留白。
    fn want_inner_w(&self) -> u16 {
        let widest = self
            .items
            .iter()
            .map(|it| it.label.width() + 3) // " k " 快捷字母列
            .max()
            .unwrap_or(0);
        let title_w = self.title.width() + 2;
        u16::try_from(widest.max(title_w) + 2).unwrap_or(u16::MAX)
    }
}

impl Overlay for PopMenu {
    fn chrome(&self) -> Chrome {
        let w = self.want_inner_w().saturating_add(2);
        // 外框最小 3 行:候选再少也保有可视的展开过程(1 行高动画退化成闪现)。
        let h = u16::try_from(self.items.len().max(1))
            .unwrap_or(u16::MAX)
            .saturating_add(2)
            .max(3);
        Chrome {
            pct_w: 0,
            pct_h: 0,
            min_w: 4,
            min_h: 3,
            max_w: w,
            max_h: h,
            animated: true,
            dock: false,
            anchor: Some((self.anchor, self.placement)),
        }
    }

    fn block(&self, _ctx: &AppState, theme: &Theme, focused: bool) -> Block<'static> {
        let border = if focused { theme.accent } else { theme.overlay };
        base_block(theme)
            .border_style(Style::new().fg(border))
            .title(Line::from(format!(" {} ", self.title)).style(Style::new().fg(theme.subtext)))
    }

    fn render_content(&self, buf: &mut Buffer, inner: Rect, _ctx: &AppState, theme: &Theme) {
        for (row, it) in self
            .items
            .iter()
            .enumerate()
            .take(usize::from(inner.height))
        {
            let Ok(dy) = u16::try_from(row) else {
                continue;
            };
            let area = Rect::new(inner.x, inner.y + dy, inner.width, 1);
            let selected = row == self.sel;
            let fg = if it.destructive {
                theme.red
            } else {
                theme.text
            };
            let mut style = Style::new().fg(fg);
            if selected {
                style = style.bg(theme.surface0).add_modifier(Modifier::BOLD);
            }
            let key_span = match it.hotkey {
                Some(k) => Span::styled(
                    format!(" {k} "),
                    Style::new().fg(theme.accent).add_modifier(Modifier::BOLD),
                ),
                None => Span::raw("   "),
            };
            let spans = vec![key_span, Span::styled(it.label.clone(), style)];
            Paragraph::new(Line::from(spans).style(if selected {
                Style::new().bg(theme.surface0)
            } else {
                Style::new()
            }))
            .render(area, buf);
        }
    }

    fn on_key(&mut self, key: &KeyEvent, _ctx: &AppState) -> OverlayResponse {
        match key.code {
            KeyCode::Esc => OverlayResponse::Do(OverlayAction::CloseTop),
            // Enter 兜底确认:默认 <CR> 经 activate 走 on_action,这里接住「<CR> 被解绑」的情形。
            KeyCode::Enter => self.confirm(),
            KeyCode::Tab | KeyCode::Down => {
                self.sel = self
                    .sel
                    .saturating_add(1)
                    .min(self.items.len().saturating_sub(1));
                OverlayResponse::Consumed
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.sel = self.sel.saturating_sub(1);
                OverlayResponse::Consumed
            }
            KeyCode::Char(c) => {
                // 快捷字母直达:命中即确认执行。
                let hit = self
                    .items
                    .iter()
                    .find(|it| it.hotkey == Some(c))
                    .map(|it| it.action.clone());
                match hit {
                    Some(a) => OverlayResponse::Do(OverlayAction::Menu(a)),
                    None => OverlayResponse::Consumed,
                }
            }
            // 模态:吞掉其余按键。
            _ => OverlayResponse::Consumed,
        }
    }

    fn on_action(&mut self, action: Action, _ctx: &AppState) -> Option<OverlayResponse> {
        let max = self.items.len().saturating_sub(1);
        match action {
            // 导航族经 keymap(跟随 j/k 重映射),钳制不循环(与 queue 浮层同手感)。
            Action::MoveSelection(mv) => {
                self.sel = match mv {
                    SelectionMove::Down(n) => self.sel.saturating_add(n).min(max),
                    SelectionMove::Up(n) => self.sel.saturating_sub(n),
                    SelectionMove::First => 0,
                    SelectionMove::Last => max,
                };
                Some(OverlayResponse::Consumed)
            }
            // 激活:activate(默认 l/<CR>)经 keymap 进来确认当前项,与列表「l 进入」同手感。
            Action::ActivateSelection => Some(self.confirm()),
            // 关闭族:back / quit 在模态菜单收敛为关闭本浮层。
            Action::BackOrClearSearch | Action::OpenQuitConfirm => {
                Some(OverlayResponse::Do(OverlayAction::CloseTop))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent};
    use mineral_model::{Song, SongId, SourceKind};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    use super::{MenuAction, MenuItem, PopMenu};
    use crate::components::popup::component::{
        Overlay, OverlayAction, OverlayResponse, render_overlay,
    };
    use crate::components::popup::placement::Placement;
    use crate::render::theme::Theme;
    use crate::runtime::action::{Action, SelectionMove};
    use crate::runtime::state::AppState;

    /// 极简测试歌(只有 id / 名字有意义)。
    fn song(name: &str) -> Box<Song> {
        Box::new(Song {
            id: SongId::new(SourceKind::LOCAL, name),
            name: name.to_owned(),
            artists: Vec::new(),
            album: None,
            duration_ms: 0,
            cover_url: None,
            source_url: None,
        })
    }

    /// 操作菜单三项(模拟 Library 歌曲上下文)。
    fn action_items() -> Vec<MenuItem> {
        vec![
            MenuItem::keyed('p', "Play next", MenuAction::PlayNext(song("s1"))),
            MenuItem::keyed('a', "Append to queue", MenuAction::Append(song("s1"))),
            MenuItem::keyed('d', "Download", MenuAction::Download(song("s1"))),
        ]
    }

    /// 锚点(屏内一行)。
    fn anchor() -> Rect {
        Rect::new(2, 1, 10, 1)
    }

    /// 导航(经全局 Action)钳制移动 + Enter 确认产出选中项动作。
    #[test]
    fn navigate_and_confirm() -> color_eyre::Result<()> {
        let ctx = AppState::test_default()?;
        let mut menu = PopMenu::new("Actions", action_items(), anchor(), Placement::Below);
        let resp = menu.on_action(Action::MoveSelection(SelectionMove::Down(1)), &ctx);
        assert!(matches!(resp, Some(OverlayResponse::Consumed)));
        let OverlayResponse::Do(OverlayAction::Menu(action)) =
            menu.on_key(&KeyEvent::from(KeyCode::Enter), &ctx)
        else {
            color_eyre::eyre::bail!("Enter 应产出菜单动作");
        };
        assert_eq!(action, MenuAction::Append(song("s1")), "j 一步后确认第二项");
        // Last → 越底钳制在末项。
        menu.on_action(Action::MoveSelection(SelectionMove::Down(99)), &ctx);
        let OverlayResponse::Do(OverlayAction::Menu(action)) =
            menu.on_key(&KeyEvent::from(KeyCode::Enter), &ctx)
        else {
            color_eyre::eyre::bail!("Enter 应产出菜单动作");
        };
        assert_eq!(action, MenuAction::Download(song("s1")), "大步越底钳到末项");
        Ok(())
    }

    /// `activate`(l/<CR>)经 on_action 确认当前项,与 Enter 兜底同效。
    #[test]
    fn activate_action_confirms_selection() -> color_eyre::Result<()> {
        let ctx = AppState::test_default()?;
        let mut menu = PopMenu::new("Actions", action_items(), anchor(), Placement::Below);
        menu.on_action(Action::MoveSelection(SelectionMove::Down(1)), &ctx);
        let Some(OverlayResponse::Do(OverlayAction::Menu(action))) =
            menu.on_action(Action::ActivateSelection, &ctx)
        else {
            color_eyre::eyre::bail!("activate 应经 on_action 产出菜单动作");
        };
        assert_eq!(
            action,
            MenuAction::Append(song("s1")),
            "确认当前选中(第二项)"
        );
        Ok(())
    }

    /// 快捷字母直达执行;未注册字母吞键不动作。
    #[test]
    fn hotkey_jumps_and_unknown_swallowed() -> color_eyre::Result<()> {
        let ctx = AppState::test_default()?;
        let mut menu = PopMenu::new("Actions", action_items(), anchor(), Placement::Below);
        let OverlayResponse::Do(OverlayAction::Menu(action)) =
            menu.on_key(&KeyEvent::from(KeyCode::Char('d')), &ctx)
        else {
            color_eyre::eyre::bail!("快捷字母应直达确认");
        };
        assert_eq!(action, MenuAction::Download(song("s1")));
        assert!(
            matches!(
                menu.on_key(&KeyEvent::from(KeyCode::Char('z')), &ctx),
                OverlayResponse::Consumed
            ),
            "未注册字母应吞键"
        );
        Ok(())
    }

    /// back / quit 动作在菜单内收敛为关闭;Esc 裸键同。
    #[test]
    fn close_paths() -> color_eyre::Result<()> {
        let ctx = AppState::test_default()?;
        let mut menu = PopMenu::new("Actions", action_items(), anchor(), Placement::Below);
        assert!(matches!(
            menu.on_action(Action::BackOrClearSearch, &ctx),
            Some(OverlayResponse::Do(OverlayAction::CloseTop))
        ));
        assert!(matches!(
            menu.on_action(Action::OpenQuitConfirm, &ctx),
            Some(OverlayResponse::Do(OverlayAction::CloseTop))
        ));
        assert!(matches!(
            menu.on_key(&KeyEvent::from(KeyCode::Esc), &ctx),
            OverlayResponse::Do(OverlayAction::CloseTop)
        ));
        Ok(())
    }

    /// 锚定渲染快照:锚点下方弹出、快捷字母列、首项高亮、危险项红色置底。
    #[test]
    fn menu_anchored_snapshot() -> color_eyre::Result<()> {
        let mut terminal = Terminal::new(TestBackend::new(40, 12))?;
        let ctx = AppState::test_default()?;
        let mut items = action_items();
        items.push(MenuItem {
            hotkey: Some('x'),
            label: "Remove from playlist".into(),
            action: MenuAction::Copy("placeholder".into()),
            destructive: true,
        });
        let menu = PopMenu::new("Actions", items, anchor(), Placement::Below);
        terminal.draw(|f| {
            render_overlay(
                f,
                f.area(),
                &menu,
                /*scale*/ 1000,
                /*focused*/ true,
                &ctx,
                &Theme::default(),
            );
        })?;
        crate::test_support::assert_snap!(
            "PopMenu 锚定弹出(锚点下方,快捷字母列,首项高亮,危险项红色置底)",
            terminal.backend()
        );
        Ok(())
    }
}

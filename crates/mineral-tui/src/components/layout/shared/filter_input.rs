//! 本地过滤列表共用的键盘编辑和身份恢复，不认识业务页面。

use crate::runtime::line_input::InputRequest;
use crate::runtime::scroll::list::ScrollList;
use crate::runtime::state::{ListExpansionState, SearchState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// 查询编辑对列表选择的影响。
pub(crate) enum FilterEdit {
    /// 查询保持不变。
    Unchanged,

    /// 新查询应从首行展示。
    Changed,

    /// 清词需要按组件提供的身份映射恢复选择。
    Clear {
        /// 删除最后一字属于连续输入，更新封面防抖时间。
        typed: bool,
    },
}

/// 处理通用文本输入；清词时由组件提供自己的身份映射。
pub(crate) fn edit(search: &mut SearchState, key: &KeyEvent) -> FilterEdit {
    match key.code {
        KeyCode::Esc => {
            search.typing = false;
            FilterEdit::Clear { typed: false }
        }
        KeyCode::Enter => {
            search.typing = false;
            FilterEdit::Unchanged
        }
        KeyCode::Backspace if search.query().is_empty() => {
            search.typing = false;
            FilterEdit::Unchanged
        }
        KeyCode::Backspace
            if search.query().chars().count() == 1 && !search.query_split().0.is_empty() =>
        {
            FilterEdit::Clear { typed: true }
        }
        KeyCode::Char(_) if key.modifiers.contains(KeyModifiers::CONTROL) => FilterEdit::Unchanged,
        KeyCode::Char(c) => {
            search.edit(InputRequest::Insert(c));
            FilterEdit::Changed
        }
        KeyCode::Backspace => {
            if search.edit(InputRequest::DeletePrev) {
                FilterEdit::Changed
            } else {
                FilterEdit::Unchanged
            }
        }
        KeyCode::Left => {
            search.edit(InputRequest::Left);
            FilterEdit::Unchanged
        }
        KeyCode::Right => {
            search.edit(InputRequest::Right);
            FilterEdit::Unchanged
        }
        KeyCode::Home => {
            search.edit(InputRequest::Home);
            FilterEdit::Unchanged
        }
        KeyCode::End => {
            search.edit(InputRequest::End);
            FilterEdit::Unchanged
        }
        _ => FilterEdit::Unchanged,
    }
}

/// 清词后按行身份保留选择及屏幕位置，动画只消费此前准备的输入。
pub(crate) fn restore<K: Clone + Eq + std::hash::Hash>(
    scroll: &mut ScrollList,
    expansion: &mut ListExpansionState<K>,
    index: Option<usize>,
    order: impl Iterator<Item = (K, usize)>,
    animation: &crate::config::AnimationConfig,
) {
    let previous_selection = scroll.sel();
    let screen_row = previous_selection.saturating_sub(scroll.scroll_target());
    if let Some(index) = index {
        scroll.place(index, screen_row);
    } else {
        scroll.place(0, 0);
    }
    mineral_log::debug!(target: "tui", previous_selection, selection = scroll.sel(), screen_row, "clear list filter preserving selected item");
    if let Some(index) = index {
        expansion.start(
            order,
            index,
            crate::render::anim::ticks16_from_ms(
                *animation.list_scroll_ms(),
                *animation.frame_tick_ms(),
            ),
        );
    }
}

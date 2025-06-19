use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

use crate::{app::Context, state::PopupState};

/// 处理全局快捷键事件。
///
/// - `'q'`：弹出退出确认对话框（设置 [`PopupState::ConfirmExit`]）
/// - `'?'`：触发帮助菜单（尚未实现）
///
/// 如果该事件被识别为全局快捷键之一，并被成功处理，
/// 则返回 `true` 表示已处理；否则返回 `false`，以继续向其他页面逻辑分发。
///
/// # 参数
///
/// - `app`：可变引用，表示当前应用状态 [`App`]，用于修改 UI 状态或响应动作。
/// - `key_event`：事件引用，表示当前待处理的终端事件。
///
/// # 返回
///
/// 返回一个布尔值，表示该事件是否被全局快捷键处理。
///
/// # 示例
///
/// ```ignore
/// if handle_global_key(&mut app, &event) {
///     return;
/// }
/// ```
///
/// # 注意
///
/// 该函数只处理 `KeyEventKind::Press` 类型的按键事件，
/// 忽略 `Release` 和 `Repeat` 类型。
pub(super) fn handle_global_key(ctx: &mut Context, key_event: &KeyEvent) -> bool {
    if KeyEventKind::Press == key_event.kind {
        match key_event.code {
            KeyCode::Char('q') => {
                ctx.popup(PopupState::ConfirmExit);
                return true;
            }
            KeyCode::Char('?') => {
                todo!("全局帮助菜单")
            }
            KeyCode::Char('n') => {
                ctx.notify_debug("Test", "发送了一个Notification");
            }
            _ => {}
        }
    }
    false
}

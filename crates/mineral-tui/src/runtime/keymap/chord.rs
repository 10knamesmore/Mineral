//! 将终端按键归一为可查表的语义和弦。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mineral_config::keys::{Key, KeyChord};

/// 把一个 crossterm 按键事件归一到 [`KeyChord`]:只保留 SHIFT / CONTROL 修饰
/// (其余视为终端噪声丢弃),字符键的 SHIFT 由 [`KeyChord`] 的构造不变量吸收。
///
/// # Params:
///   - `key`: crossterm 按键事件
///
/// # Return:
///   keymap 关心的键给 `Some`;F 键 / Home 等未建模的键给 `None`(查表必 miss)
pub fn chord_from_event(key: &KeyEvent) -> Option<KeyChord> {
    let semantic = match key.code {
        KeyCode::Char(c) => Key::Char(c),
        KeyCode::Left => Key::Left,
        KeyCode::Right => Key::Right,
        KeyCode::Up => Key::Up,
        KeyCode::Down => Key::Down,
        KeyCode::Tab => Key::Tab,
        KeyCode::Enter => Key::Enter,
        KeyCode::Esc => Key::Esc,
        KeyCode::Backspace => Key::Backspace,
        _ => return None,
    };
    let mut chord = if key.modifiers.contains(KeyModifiers::SHIFT) {
        KeyChord::shifted(semantic)
    } else {
        KeyChord::plain(semantic)
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        chord = chord.with_ctrl();
    }
    Some(chord)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mineral_config::keys::KeyChord;

    use super::chord_from_event;

    #[test]
    fn chord_normalizes_modifiers() -> color_eyre::Result<()> {
        // 大写字符自带 SHIFT 位:归一后等于纯 'J'。
        let ev = KeyEvent::new(KeyCode::Char('J'), KeyModifiers::SHIFT);
        assert_eq!(chord_from_event(&ev), Some(KeyChord::parse("J")?));
        // 终端把 `+` 报成 Shift+'+':SHIFT 应被字符键吸收。
        let ev = KeyEvent::new(KeyCode::Char('+'), KeyModifiers::SHIFT);
        assert_eq!(chord_from_event(&ev), Some(KeyChord::parse("+")?));
        // 非字符键保留 SHIFT。
        let ev = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(chord_from_event(&ev), Some(KeyChord::parse("<S-Left>")?));
        // 无关修饰(如 META)丢弃,不影响命中。
        let ev = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::META);
        assert_eq!(chord_from_event(&ev), Some(KeyChord::parse("j")?));
        // 未建模的键(Home / F 键)归一不出和弦。
        let ev = KeyEvent::new(KeyCode::Home, KeyModifiers::empty());
        assert_eq!(chord_from_event(&ev), None);
        Ok(())
    }
}

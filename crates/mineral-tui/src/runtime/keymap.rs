//! 键 → 动作绑定表([`Keymap`])与 crossterm 事件到 [`KeyChord`] 的归一。
//!
//! 表内容本期写死([`Keymap::builtin`]);config 注入缝是 [`Keymap::from_entries`],
//! 后续把「default.lua + 用户 lua 解析出的绑定」喂进来即可,本模块不接任何 config。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mineral_config::keys::{Key, KeyChord};
use rustc_hash::FxHashMap;

use super::action::{Action, SeekDelta, SelectionMove, VolumeDelta};

/// 音量步长(百分点);`+`/`-` 一次。
const VOLUME_STEP: VolumeDelta = VolumeDelta(5);

/// 普通 seek 步长(秒);`←`/`→` 一次。
const SEEK_STEP: SeekDelta = SeekDelta(5);

/// 大跨度 seek 步长(秒);`Shift+←`/`Shift+→` 一次。
const SEEK_BIG_STEP: SeekDelta = SeekDelta(30);

/// 大跨度跳行步长(行);`J`/`K` 一次。j/k/箭头仍是 1。
const ROW_BIG_STEP: usize = 7;

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

/// 键 → 动作绑定表。本期 [`Self::builtin`] 写死;Phase 0 后续 PR 用
/// [`Self::from_entries`] 喂入「default.lua + 用户 lua 解析出的绑定」。
pub struct Keymap {
    /// 归一和弦 → 动作。一对一(单动作);多键映同动作即多条目。
    table: FxHashMap<KeyChord, Action>,
}

impl Keymap {
    /// 内建默认绑定(逐键对齐重构前 `app.rs` 的散落 match)。
    ///
    /// # Return:
    ///   完整默认表
    pub fn builtin() -> Self {
        let plain = |c: char| KeyChord::plain(Key::Char(c));
        Self::from_entries([
            // ---- 全局 ----
            (plain('z'), Action::ToggleFullscreen),
            (KeyChord::plain(Key::Tab), Action::OpenQueue),
            (plain('q'), Action::OpenQuitConfirm),
            (plain('t'), Action::CycleLyricExtra),
            (plain('/'), Action::EnterSearch),
            // ---- 播放控制 ----
            (plain(' '), Action::TogglePlayPause),
            (plain('m'), Action::CyclePlayMode),
            (plain('+'), Action::NudgeVolume(VOLUME_STEP)),
            (plain('='), Action::NudgeVolume(VOLUME_STEP)),
            (plain('-'), Action::NudgeVolume(VolumeDelta(-VOLUME_STEP.0))),
            (plain('_'), Action::NudgeVolume(VolumeDelta(-VOLUME_STEP.0))),
            (
                KeyChord::plain(Key::Left),
                Action::SeekRelative(SeekDelta(-SEEK_STEP.0)),
            ),
            (KeyChord::plain(Key::Right), Action::SeekRelative(SEEK_STEP)),
            (
                KeyChord::shifted(Key::Left),
                Action::SeekRelative(SeekDelta(-SEEK_BIG_STEP.0)),
            ),
            (
                KeyChord::shifted(Key::Right),
                Action::SeekRelative(SEEK_BIG_STEP),
            ),
            (plain('p'), Action::PrevOrRestart),
            (plain('n'), Action::NextSong),
            // ---- 列表视图 ----
            (plain('j'), Action::MoveSelection(SelectionMove::Down(1))),
            (
                KeyChord::plain(Key::Down),
                Action::MoveSelection(SelectionMove::Down(1)),
            ),
            (plain('k'), Action::MoveSelection(SelectionMove::Up(1))),
            (
                KeyChord::plain(Key::Up),
                Action::MoveSelection(SelectionMove::Up(1)),
            ),
            (
                plain('J'),
                Action::MoveSelection(SelectionMove::Down(ROW_BIG_STEP)),
            ),
            (
                plain('K'),
                Action::MoveSelection(SelectionMove::Up(ROW_BIG_STEP)),
            ),
            (plain('g'), Action::MoveSelection(SelectionMove::First)),
            (plain('G'), Action::MoveSelection(SelectionMove::Last)),
            (plain('l'), Action::ActivateSelection),
            (KeyChord::plain(Key::Enter), Action::ActivateSelection),
            (plain('h'), Action::BackOrClearSearch),
            (KeyChord::plain(Key::Esc), Action::BackOrClearSearch),
            (KeyChord::plain(Key::Backspace), Action::BackOrClearSearch),
            (plain('f'), Action::ToggleLoveSelection),
            (plain('d'), Action::DownloadSelection),
        ])
    }

    /// 从外部绑定构造(config 注入缝;本期除 [`Self::builtin`] 外无调用方)。
    ///
    /// # Params:
    ///   - `entries`: 和弦 → 动作绑定;重复和弦后写覆盖先写
    ///
    /// # Return:
    ///   查表结构
    pub fn from_entries(entries: impl IntoIterator<Item = (KeyChord, Action)>) -> Self {
        Self {
            table: entries.into_iter().collect::<FxHashMap<_, _>>(),
        }
    }

    /// 查表。
    ///
    /// # Params:
    ///   - `chord`: 归一后的按键和弦
    ///
    /// # Return:
    ///   命中给对应 [`Action`],未绑定给 `None`
    pub fn lookup(&self, chord: KeyChord) -> Option<Action> {
        self.table.get(&chord).copied()
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use mineral_config::keys::KeyChord;

    use super::super::action::{Action, SeekDelta, SelectionMove, VolumeDelta};
    use super::{Keymap, chord_from_event};

    /// 默认表的全部预期绑定(键字符串 → 动作),与重构前 `app.rs` 散落 match 逐键对齐。
    /// 既是 `builtin_maps_every_known_key` 的断言源,也是快照 dump 的输入。
    fn expected_bindings() -> Vec<(&'static str, Action)> {
        vec![
            // ---- 全局(handle_key 直连段) ----
            ("z", Action::ToggleFullscreen),
            ("tab", Action::OpenQueue),
            ("q", Action::OpenQuitConfirm),
            ("t", Action::CycleLyricExtra),
            ("/", Action::EnterSearch),
            // ---- 播放控制(handle_playback_key) ----
            ("space", Action::TogglePlayPause),
            ("m", Action::CyclePlayMode),
            ("+", Action::NudgeVolume(VolumeDelta(5))),
            ("=", Action::NudgeVolume(VolumeDelta(5))),
            ("-", Action::NudgeVolume(VolumeDelta(-5))),
            ("_", Action::NudgeVolume(VolumeDelta(-5))),
            ("Left", Action::SeekRelative(SeekDelta(-5))),
            ("Right", Action::SeekRelative(SeekDelta(5))),
            ("Shift+Left", Action::SeekRelative(SeekDelta(-30))),
            ("Shift+Right", Action::SeekRelative(SeekDelta(30))),
            ("p", Action::PrevOrRestart),
            ("n", Action::NextSong),
            // ---- 列表视图(handle_playlists_key / handle_library_key 归一) ----
            ("j", Action::MoveSelection(SelectionMove::Down(1))),
            ("Down", Action::MoveSelection(SelectionMove::Down(1))),
            ("k", Action::MoveSelection(SelectionMove::Up(1))),
            ("Up", Action::MoveSelection(SelectionMove::Up(1))),
            ("J", Action::MoveSelection(SelectionMove::Down(7))),
            ("K", Action::MoveSelection(SelectionMove::Up(7))),
            ("g", Action::MoveSelection(SelectionMove::First)),
            ("G", Action::MoveSelection(SelectionMove::Last)),
            ("l", Action::ActivateSelection),
            ("enter", Action::ActivateSelection),
            ("h", Action::BackOrClearSearch),
            ("esc", Action::BackOrClearSearch),
            ("backspace", Action::BackOrClearSearch),
            ("f", Action::ToggleLoveSelection),
            ("d", Action::DownloadSelection),
        ]
    }

    #[test]
    fn builtin_maps_every_known_key() -> color_eyre::Result<()> {
        let km = Keymap::builtin();
        let expected = expected_bindings();
        for (s, action) in &expected {
            let chord = KeyChord::parse(s)?;
            assert_eq!(km.lookup(chord), Some(*action), "绑定 `{s}` 不符");
        }
        // 表里没有多余条目(逐键对齐 = 双向)。
        assert_eq!(km.table.len(), expected.len(), "默认表条目数不符");
        Ok(())
    }

    #[test]
    fn unbound_key_returns_none() -> color_eyre::Result<()> {
        let km = Keymap::builtin();
        assert_eq!(km.lookup(KeyChord::parse("!")?), None);
        assert_eq!(km.lookup(KeyChord::parse("x")?), None);
        Ok(())
    }

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
        assert_eq!(chord_from_event(&ev), Some(KeyChord::parse("Shift+Left")?));
        // 无关修饰(如 META)丢弃,不影响命中。
        let ev = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::META);
        assert_eq!(chord_from_event(&ev), Some(KeyChord::parse("j")?));
        // 未建模的键(Home / F 键)归一不出和弦。
        let ev = KeyEvent::new(KeyCode::Home, KeyModifiers::empty());
        assert_eq!(chord_from_event(&ev), None);
        Ok(())
    }

    #[test]
    fn builtin_table_snapshot() {
        let km = Keymap::builtin();
        let mut lines = km
            .table
            .iter()
            .map(|(chord, action)| format!("{chord} → {action:?}"))
            .collect::<Vec<String>>();
        lines.sort();
        crate::test_support::assert_snap!(
            "默认键位绑定表(和弦 → 动作,字典序;config 落地前的唯一真相源)",
            lines.join("\n")
        );
    }
}

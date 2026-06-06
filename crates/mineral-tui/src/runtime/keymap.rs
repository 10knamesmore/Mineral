//! 键 → 动作绑定表([`Keymap`])与 crossterm 事件到 [`KeyChord`] 的归一。
//!
//! 表内容由配置落地([`Keymap::from_config`]):keys 段给「动作 → 键」绑定,
//! behavior 段给带参动作的步长;default.lua 与用户 lua 已在 loader 深合并。

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use mineral_config::keys::{Key, KeyChord};
use rustc_hash::FxHashMap;

use super::action::{Action, ScriptSlot, SeekDelta, SelectionMove, VolumeDelta};

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

/// 键 → 动作绑定表。生产路径经 [`Self::from_config`] 由配置落地;
/// [`Self::from_entries`] 是底层构造缝(测试 / 自定义表直喂)。
pub struct Keymap {
    /// 归一和弦 → 动作。一对一(单动作);多键映同动作即多条目。
    table: FxHashMap<KeyChord, Action>,

    /// 脚本动作名表:`Action::InvokeScript` 的槽位 → 注册名
    /// (Action 须 `Copy`,名字进不了枚举,经此表间接)。
    script_names: Vec<String>,
}

impl Keymap {
    /// 从配置落地键表:keys 段给「动作 → 键」绑定,behavior 段给带参动作的步长。
    /// 数值经 `From` 拓宽到 Action 参数类型(无 `as`)。
    ///
    /// # Params:
    ///   - `keys`: 键位重映射段(动作 → 键,深合并后产物)
    ///   - `behavior`: 交互手感段(volume/seek 步长、列表大步行数)
    ///
    /// # Return:
    ///   查表结构。
    pub fn from_config(
        keys: &mineral_config::KeysConfig,
        behavior: &mineral_config::BehaviorConfig,
    ) -> Self {
        let vol = i16::from(*behavior.volume_step());
        let seek = i64::from(*behavior.seek_step_secs());
        let seek_big = i64::from(*behavior.seek_big_step_secs());
        let jump = usize::from(*behavior.list_jump_rows());
        // 脚本动作绑定:开放映射按名排序保证槽位确定性。
        let mut script_bindings = keys.script().iter().collect::<Vec<_>>();
        script_bindings.sort_by(|a, b| a.0.cmp(b.0));
        let mut script_names = Vec::with_capacity(script_bindings.len());
        let mut pairs: Vec<(&mineral_config::KeyBinding, Action)> = vec![
            (keys.toggle_fullscreen(), Action::ToggleFullscreen),
            (keys.open_queue(), Action::OpenQueue),
            (keys.quit(), Action::OpenQuitConfirm),
            (keys.cycle_lyric(), Action::CycleLyricExtra),
            (keys.enter_search(), Action::EnterSearch),
            (keys.play_pause(), Action::TogglePlayPause),
            (keys.cycle_mode(), Action::CyclePlayMode),
            (keys.volume_up(), Action::NudgeVolume(VolumeDelta(vol))),
            (keys.volume_down(), Action::NudgeVolume(VolumeDelta(-vol))),
            (keys.seek_forward(), Action::SeekRelative(SeekDelta(seek))),
            (keys.seek_backward(), Action::SeekRelative(SeekDelta(-seek))),
            (
                keys.seek_forward_big(),
                Action::SeekRelative(SeekDelta(seek_big)),
            ),
            (
                keys.seek_backward_big(),
                Action::SeekRelative(SeekDelta(-seek_big)),
            ),
            (keys.prev(), Action::PrevOrRestart),
            (keys.next(), Action::NextSong),
            (
                keys.move_down(),
                Action::MoveSelection(SelectionMove::Down(1)),
            ),
            (keys.move_up(), Action::MoveSelection(SelectionMove::Up(1))),
            (
                keys.move_down_big(),
                Action::MoveSelection(SelectionMove::Down(jump)),
            ),
            (
                keys.move_up_big(),
                Action::MoveSelection(SelectionMove::Up(jump)),
            ),
            (
                keys.move_first(),
                Action::MoveSelection(SelectionMove::First),
            ),
            (keys.move_last(), Action::MoveSelection(SelectionMove::Last)),
            (keys.activate(), Action::ActivateSelection),
            (keys.back(), Action::BackOrClearSearch),
            (keys.love(), Action::ToggleLoveSelection),
            (keys.download(), Action::DownloadSelection),
        ];
        for (name, binding) in script_bindings {
            pairs.push((
                binding,
                Action::InvokeScript(ScriptSlot(script_names.len())),
            ));
            script_names.push(name.clone());
        }
        let mut keymap = Self::from_entries(pairs.into_iter().flat_map(|(binding, action)| {
            binding.chords().iter().copied().map(move |c| (c, action))
        }));
        keymap.script_names = script_names;
        keymap
    }

    /// 从绑定序列构造底层查表(供 [`Self::from_config`] 与测试直喂)。
    ///
    /// # Params:
    ///   - `entries`: 和弦 → 动作绑定;重复和弦后写覆盖先写
    ///
    /// # Return:
    ///   查表结构
    pub fn from_entries(entries: impl IntoIterator<Item = (KeyChord, Action)>) -> Self {
        Self {
            table: entries.into_iter().collect::<FxHashMap<_, _>>(),
            script_names: Vec::new(),
        }
    }

    /// 槽位 → 脚本动作注册名(`Action::InvokeScript` 的执行点用)。
    ///
    /// # Params:
    ///   - `slot`: 查表命中的槽位
    ///
    /// # Return:
    ///   对应注册名;槽位越界(理论不可达)为 `None`。
    pub fn script_action(&self, slot: ScriptSlot) -> Option<&str> {
        self.script_names.get(slot.0).map(String::as_str)
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

    /// 取 defaults 配置落地的键表(= 旧 builtin 表,由 default.lua keys/behavior 驱动)。
    fn default_keymap() -> color_eyre::Result<Keymap> {
        let cfg = mineral_config::Config::defaults()?;
        Ok(Keymap::from_config(cfg.tui().keys(), cfg.tui().behavior()))
    }

    #[test]
    fn builtin_maps_every_known_key() -> color_eyre::Result<()> {
        let km = default_keymap()?;
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
        let km = default_keymap()?;
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
    fn builtin_table_snapshot() -> color_eyre::Result<()> {
        let km = default_keymap()?;
        let mut lines = km
            .table
            .iter()
            .map(|(chord, action)| format!("{chord} → {action:?}"))
            .collect::<Vec<String>>();
        lines.sort();
        crate::test_support::assert_snap!(
            "默认键位绑定表(和弦 → 动作,字典序;default.lua keys/behavior 落地产物)",
            lines.join("\n")
        );
        Ok(())
    }

    /// behavior 步长逐旋钮生效:注入 volume_step=10 / seek=15 / jump=3,Action 参数跟着变。
    #[test]
    fn behavior_steps_take_effect() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let user = dir.path().join("config.lua");
        std::fs::write(
            &user,
            "return { tui = { behavior = { volume_step = 10, seek_step_secs = 15, list_jump_rows = 3 } } }",
        )?;
        let (cfg, warnings) = mineral_config::load(&user)?;
        assert!(warnings.is_empty(), "合法配置不应有 warning: {warnings:?}");
        let km = Keymap::from_config(cfg.tui().keys(), cfg.tui().behavior());
        assert_eq!(
            km.lookup(KeyChord::parse("+")?),
            Some(Action::NudgeVolume(VolumeDelta(10)))
        );
        assert_eq!(
            km.lookup(KeyChord::parse("Left")?),
            Some(Action::SeekRelative(SeekDelta(-15)))
        );
        assert_eq!(
            km.lookup(KeyChord::parse("J")?),
            Some(Action::MoveSelection(SelectionMove::Down(3)))
        );
        Ok(())
    }

    /// keys 重映射生效:play_pause 改绑 "x" 后,space 不再命中、x 命中(数组整体替换)。
    #[test]
    fn key_remap_takes_effect() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let user = dir.path().join("config.lua");
        std::fs::write(&user, "return { tui = { keys = { play_pause = \"x\" } } }")?;
        let (cfg, warnings) = mineral_config::load(&user)?;
        assert!(warnings.is_empty(), "合法配置不应有 warning: {warnings:?}");
        let km = Keymap::from_config(cfg.tui().keys(), cfg.tui().behavior());
        assert_eq!(
            km.lookup(KeyChord::parse("x")?),
            Some(Action::TogglePlayPause)
        );
        assert_eq!(km.lookup(KeyChord::parse("space")?), None, "旧键被整体替换");
        Ok(())
    }
}

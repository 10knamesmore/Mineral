//! 从键位配置与交互步长构建动作查表和帮助目录。
//!
//! keys 段给出动作绑定，behavior 段给出带参动作的步长；tui-default.lua 与用户配置已完成深合并。

use std::borrow::Cow;

use crate::config::key_syntax::KeyChord;
use rustc_hash::FxHashMap;

use super::help::{CatalogBuilder, HelpEntry, HelpGroup};
use crate::runtime::action::{Action, ScrollStep, SeekDelta, SelectionMove, VolumeDelta};

/// 键 → 动作绑定表。生产路径经 [`Self::from_config`] 由配置落地。
pub struct Keymap {
    /// 归一和弦 → 动作。一对一(单动作);多键映同动作即多条目。
    table: FxHashMap<KeyChord, Action>,

    /// cheatsheet 目录(与查表同源产出,显示顺序;测试直喂构造时为空)。
    help: Vec<HelpEntry>,
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
        keys: &crate::config::KeysConfig,
        behavior: &crate::config::BehaviorConfig,
    ) -> Self {
        let vol = i16::from(*behavior.volume_step());
        let seek = i64::from(*behavior.seek_step_secs());
        let seek_big = i64::from(*behavior.seek_big_step_secs());
        let jump = usize::from(*behavior.list_jump_rows());
        // 绑定 × 动作 × 目录元数据(组 + label):声明序即 cheatsheet 显示序;
        // 相邻同(组, label)的成对动作(±增减 / 上下移动)在目录里合并为一行。
        type Pair<'k> = (
            &'k crate::config::KeyBinding,
            Action,
            HelpGroup,
            Cow<'static, str>,
        );
        // 每行一条绑定:`keys 字段 => Action 变体(参数), label;`——label 是任意
        // `Into<Cow>` 表达式(字面量零分配,带 behavior 实值的用 format!)。
        // 展开为单个 vec 字面量(一次分配,长度编译期已知)。
        macro_rules! bind {
            ( $( $group:ident {
                $( $getter:ident => $variant:ident $( ( $($arg:expr),+ ) )?, $label:expr; )+
            } )+ ) => { vec![ $( $(
                (
                    keys.$getter(),
                    Action::$variant $( ( $($arg),+ ) )?,
                    HelpGroup::$group,
                    $label.into(),
                )
            ),+ ),+ ] };
        }
        let pairs: Vec<Pair<'_>> = bind! {
            Playback {
                play_pause => TogglePlayPause, "Play / Pause";
                next => NextSong, "Next / Previous";
                prev => PrevOrRestart, "Next / Previous";
                cycle_mode => CyclePlayMode, "Cycle play mode";
                volume_up => NudgeVolume(VolumeDelta(vol)), format!("Volume ±{vol}");
                volume_down => NudgeVolume(VolumeDelta(-vol)), format!("Volume ±{vol}");
                seek_backward => SeekRelative(SeekDelta(-seek)), format!("Seek ±{seek}s");
                seek_forward => SeekRelative(SeekDelta(seek)), format!("Seek ±{seek}s");
                seek_backward_big => SeekRelative(SeekDelta(-seek_big)), format!("Seek ±{seek_big}s");
                seek_forward_big => SeekRelative(SeekDelta(seek_big)), format!("Seek ±{seek_big}s");
            }
            Navigate {
                move_down => MoveSelection(SelectionMove::Down(1)), "Move down / up";
                move_up => MoveSelection(SelectionMove::Up(1)), "Move down / up";
                move_down_big => MoveSelection(SelectionMove::Down(jump)), format!("Jump {jump} rows");
                move_up_big => MoveSelection(SelectionMove::Up(jump)), format!("Jump {jump} rows");
                move_first => MoveSelection(SelectionMove::First), "First / last";
                move_last => MoveSelection(SelectionMove::Last), "First / last";
                activate => ActivateSelection, "Activate";
                back => BackOrClearSearch, "Back";
                drill_into => DrillIntoSelection, "Drill into";
                cycle_detail_section => CycleDetailSection, "Cycle section";
            }
            Actions {
                love => ToggleLoveSelection, "Love";
                reorder_down => ReorderSelection(SelectionMove::Down(1)), "Move item down / up";
                reorder_up => ReorderSelection(SelectionMove::Up(1)), "Move item down / up";
                jump_to_current => JumpToCurrent, "Jump to playing";
                download => DownloadSelection, "Download";
                open_action_menu => OpenActionMenu, "Actions menu";
                open_copy_menu => OpenCopyMenu, "Copy menu";
                dismiss_notice => DismissNotice, "Dismiss notice";
            }
            View {
                toggle_fullscreen => ToggleFullscreen, "Fullscreen";
                open_search => OpenSearchView, "Search view";
                open_queue => OpenQueue, "Queue";
                open_downloads => OpenDownloads, "Downloads";
                open_audio_settings => OpenAudioSettings, "Audio settings";
                enter_search => EnterSearch, "Search input";
                cycle_lyric => CycleLyricExtra, "Lyric language";
                quit => OpenQuitConfirm, "Quit";
                open_help => OpenHelp, "This help";
            }
            Scroll {
                scroll_line_down => Scroll(ScrollStep::LineDown), "Line scroll";
                scroll_line_up => Scroll(ScrollStep::LineUp), "Line scroll";
                scroll_page_down => Scroll(ScrollStep::PageDown), "Page scroll";
                scroll_page_up => Scroll(ScrollStep::PageUp), "Page scroll";
            }
        };
        let mut table = FxHashMap::default();
        let mut catalog = CatalogBuilder::default();
        for (binding, action, group, label) in pairs {
            for chord in binding.chords() {
                // 重复和弦后写覆盖先写(与 from_entries 语义一致)。
                table.insert(*chord, action);
            }
            catalog.push(group, label, binding.chords());
        }
        Self {
            table,
            help: catalog.finish(),
        }
    }

    /// 从绑定序列构造底层查表(测试直喂;生产路径一律 [`Self::from_config`])。
    ///
    /// # Params:
    ///   - `entries`: 和弦 → 动作绑定;重复和弦后写覆盖先写
    ///
    /// # Return:
    ///   查表结构(help 目录为空)
    #[cfg(test)]
    pub fn from_entries(entries: impl IntoIterator<Item = (KeyChord, Action)>) -> Self {
        Self {
            table: entries.into_iter().collect::<FxHashMap<_, _>>(),
            help: Vec::new(),
        }
    }

    /// cheatsheet 目录(显示顺序;测试直喂构造时为空)。
    pub fn help(&self) -> &[HelpEntry] {
        &self.help
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

    /// 反查某动作绑定的键(UI 提示用,如卡片底边的关闭键)。多键绑同一动作时
    /// 取显示串字典序最小的一个,保证提示跨帧 / 跨次启动稳定。
    ///
    /// # Params:
    ///   - `action`: 目标动作
    ///
    /// # Return:
    ///   绑定键和弦;该动作未绑定任何键为 `None`。
    pub fn hint_chord(&self, action: Action) -> Option<KeyChord> {
        self.table
            .iter()
            .filter(|(_, a)| **a == action)
            .map(|(c, _)| (c.to_string(), *c))
            .min_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, c)| c)
    }
}

#[cfg(test)]
mod tests {
    use crate::config::key_syntax::KeyChord;

    use super::Keymap;
    use crate::runtime::action::{Action, ScrollStep, SeekDelta, SelectionMove, VolumeDelta};

    /// 默认键位与动作的完整对应表。
    fn expected_bindings() -> Vec<(&'static str, Action)> {
        vec![
            // ---- 全局(handle_key 直连段) ----
            ("z", Action::ToggleFullscreen),
            ("s", Action::OpenSearchView),
            ("<Tab>", Action::OpenQueue),
            ("D", Action::OpenDownloads),
            ("A", Action::OpenAudioSettings),
            ("q", Action::OpenQuitConfirm),
            ("t", Action::CycleLyricExtra),
            ("/", Action::EnterSearch),
            ("?", Action::OpenHelp),
            // ---- 播放控制(handle_playback_key) ----
            ("<Space>", Action::TogglePlayPause),
            ("m", Action::CyclePlayMode),
            ("+", Action::NudgeVolume(VolumeDelta(5))),
            ("=", Action::NudgeVolume(VolumeDelta(5))),
            ("-", Action::NudgeVolume(VolumeDelta(-5))),
            ("_", Action::NudgeVolume(VolumeDelta(-5))),
            ("<Left>", Action::SeekRelative(SeekDelta(-5))),
            ("<Right>", Action::SeekRelative(SeekDelta(5))),
            ("<S-Left>", Action::SeekRelative(SeekDelta(-30))),
            ("<S-Right>", Action::SeekRelative(SeekDelta(30))),
            ("p", Action::PrevOrRestart),
            ("n", Action::NextSong),
            // ---- 列表视图(handle_playlists_key / handle_library_key 归一) ----
            ("j", Action::MoveSelection(SelectionMove::Down(1))),
            ("<Down>", Action::MoveSelection(SelectionMove::Down(1))),
            ("k", Action::MoveSelection(SelectionMove::Up(1))),
            ("<Up>", Action::MoveSelection(SelectionMove::Up(1))),
            ("J", Action::MoveSelection(SelectionMove::Down(7))),
            ("K", Action::MoveSelection(SelectionMove::Up(7))),
            ("g", Action::MoveSelection(SelectionMove::First)),
            ("G", Action::MoveSelection(SelectionMove::Last)),
            ("l", Action::ActivateSelection),
            ("<CR>", Action::ActivateSelection),
            ("h", Action::BackOrClearSearch),
            ("<Esc>", Action::BackOrClearSearch),
            ("<BS>", Action::BackOrClearSearch),
            ("<C-h>", Action::BackOrClearSearch),
            ("<C-l>", Action::DrillIntoSelection),
            ("[", Action::CycleDetailSection),
            ("]", Action::CycleDetailSection),
            ("f", Action::ToggleLoveSelection),
            ("<C-j>", Action::ReorderSelection(SelectionMove::Down(1))),
            ("<C-k>", Action::ReorderSelection(SelectionMove::Up(1))),
            ("c", Action::JumpToCurrent),
            ("d", Action::DownloadSelection),
            ("x", Action::DismissNotice),
            ("o", Action::OpenActionMenu),
            ("y", Action::OpenCopyMenu),
            // ---- 全屏歌词手动滚动:单行档 = nvim halfpage 键,多行档 = fullpage 键 ----
            ("<C-d>", Action::Scroll(ScrollStep::LineDown)),
            ("<C-u>", Action::Scroll(ScrollStep::LineUp)),
            ("<C-f>", Action::Scroll(ScrollStep::PageDown)),
            ("<C-b>", Action::Scroll(ScrollStep::PageUp)),
        ]
    }

    /// 取 defaults 配置落地的键表(= 旧 builtin 表,由 tui-default.lua keys/behavior 驱动)。
    fn default_keymap() -> color_eyre::Result<Keymap> {
        let cfg = crate::config::TuiConfig::defaults()?;
        Ok(Keymap::from_config(cfg.keys(), cfg.behavior()))
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

    /// 键反查:默认表 DismissNotice → "x";多键动作(activate = l/<CR>)取字典序
    /// 最小的显示串,提示稳定;未绑定动作反查无果。
    #[test]
    fn hint_chord_reverse_lookup() -> color_eyre::Result<()> {
        let km = default_keymap()?;
        assert_eq!(
            km.hint_chord(Action::DismissNotice),
            Some(KeyChord::parse("x")?)
        );
        assert_eq!(
            km.hint_chord(Action::ActivateSelection),
            Some(KeyChord::parse("<CR>")?),
            "\"<CR>\" 字典序小于 \"l\""
        );
        let empty = Keymap::from_entries(std::iter::empty());
        assert_eq!(empty.hint_chord(Action::DismissNotice), None);
        Ok(())
    }

    /// behavior 步长逐旋钮生效:注入 volume_step=10 / seek=15 / jump=3,Action 参数跟着变。
    #[test]
    fn behavior_steps_take_effect() -> color_eyre::Result<()> {
        let dir = tempfile::tempdir()?;
        let user = dir.path().join("tui.lua");
        std::fs::write(
            &user,
            "return { behavior = { volume_step = 10, seek_step_secs = 15, list_jump_rows = 3 } }",
        )?;
        let (cfg, warnings) = crate::config::load_tui(&user)?;
        assert!(warnings.is_empty(), "合法配置不应有 warning: {warnings:?}");
        let km = Keymap::from_config(cfg.keys(), cfg.behavior());
        assert_eq!(
            km.lookup(KeyChord::parse("+")?),
            Some(Action::NudgeVolume(VolumeDelta(10)))
        );
        assert_eq!(
            km.lookup(KeyChord::parse("<Left>")?),
            Some(Action::SeekRelative(SeekDelta(-15)))
        );
        assert_eq!(
            km.lookup(KeyChord::parse("J")?),
            Some(Action::MoveSelection(SelectionMove::Down(3)))
        );
        Ok(())
    }
}

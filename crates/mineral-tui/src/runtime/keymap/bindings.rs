//! 从键位配置与交互步长构建动作查表、脚本槽位和帮助目录。
//!
//! keys 段给出动作绑定，behavior 段给出带参动作的步长；default.lua 与用户配置已完成深合并。

use std::borrow::Cow;

use mineral_config::keys::KeyChord;
use rustc_hash::FxHashMap;

use super::help::{CatalogBuilder, HelpEntry, HelpGroup};
use crate::runtime::action::{
    Action, ScriptSlot, ScrollStep, SeekDelta, SelectionMove, VolumeDelta,
};

/// 键 → 动作绑定表。生产路径经 [`Self::from_config`] 由配置落地。
pub struct Keymap {
    /// 归一和弦 → 动作。一对一(单动作);多键映同动作即多条目。
    table: FxHashMap<KeyChord, Action>,

    /// 脚本动作名表:`Action::InvokeScript` 的槽位 → 注册名
    /// (Action 须 `Copy`,名字进不了枚举,经此表间接)。
    script_names: Vec<String>,

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
        // 绑定 × 动作 × 目录元数据(组 + label):声明序即 cheatsheet 显示序;
        // 相邻同(组, label)的成对动作(±增减 / 上下移动)在目录里合并为一行。
        type Pair<'k> = (
            &'k mineral_config::KeyBinding,
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
        let mut pairs: Vec<Pair<'_>> = bind! {
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
        for (name, binding) in script_bindings {
            pairs.push((
                binding,
                Action::InvokeScript(ScriptSlot(script_names.len())),
                HelpGroup::Scripts,
                Cow::Owned(name.clone()),
            ));
            script_names.push(name.clone());
        }
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
            script_names,
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
            script_names: Vec::new(),
            help: Vec::new(),
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

    /// 把 daemon 拉回的 `mineral.bind` 表追加进查表(槽位排在配置
    /// `keys.script` 之后)。键字符串解析失败的条目 warn 跳过、不占槽位,
    /// 不拖死其余绑定。
    ///
    /// # Params:
    ///   - `binds`: bind 表(注册顺序)
    pub fn append_script_binds(&mut self, binds: &[mineral_protocol::ScriptBind]) {
        for bind in binds {
            let chord = match mineral_config::keys::KeyChord::parse(&bind.key) {
                Ok(chord) => chord,
                Err(e) => {
                    mineral_log::warn!(
                        target: "tui",
                        key = bind.key,
                        action = bind.action,
                        error = mineral_log::chain(&e),
                        "mineral.bind 的键解析失败,跳过该绑定"
                    );
                    continue;
                }
            };
            self.table.insert(
                chord,
                Action::InvokeScript(ScriptSlot(self.script_names.len())),
            );
            self.help.push(HelpEntry::script(&bind.action, chord));
            self.script_names.push(bind.action.clone());
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
    use mineral_config::keys::KeyChord;

    use super::Keymap;
    use crate::runtime::action::{Action, ScrollStep, SeekDelta, SelectionMove, VolumeDelta};

    /// 默认表的全部预期绑定(键字符串 → 动作),与重构前 `app.rs` 散落 match 逐键对齐。
    /// 既是 `builtin_maps_every_known_key` 的断言源,也是快照 dump 的输入。
    fn expected_bindings() -> Vec<(&'static str, Action)> {
        vec![
            // ---- 全局(handle_key 直连段) ----
            ("z", Action::ToggleFullscreen),
            ("s", Action::OpenSearchView),
            ("<Tab>", Action::OpenQueue),
            ("D", Action::OpenDownloads),
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
            km.lookup(KeyChord::parse("<Left>")?),
            Some(Action::SeekRelative(SeekDelta(-15)))
        );
        assert_eq!(
            km.lookup(KeyChord::parse("J")?),
            Some(Action::MoveSelection(SelectionMove::Down(3)))
        );
        Ok(())
    }

    /// daemon 拉回的 bind 表合进 keymap:键命中 InvokeScript 新槽位、槽位
    /// 解析回内部名;非法键字符串跳过该条不拖死其余。
    #[test]
    fn script_binds_append_after_config_slots() -> color_eyre::Result<()> {
        use mineral_protocol::ScriptBind;

        use crate::runtime::action::ScriptSlot;
        let dir = tempfile::tempdir()?;
        let user = dir.path().join("config.lua");
        // 配置里已有一个 keys.script 槽位,bind 槽位必须排在其后不串位。
        std::fs::write(
            &user,
            "return { tui = { keys = { script = { [\"my.first\"] = \"X\" } } } }",
        )?;
        let (cfg, warnings) = mineral_config::load(&user)?;
        assert!(warnings.is_empty(), "合法配置不应有 warning: {warnings:?}");
        let mut km = Keymap::from_config(cfg.tui().keys(), cfg.tui().behavior());
        km.append_script_binds(&[
            ScriptBind {
                key: "<C-g>".to_owned(),
                action: "bind#1".to_owned(),
            },
            ScriptBind {
                key: "不是键".to_owned(),
                action: "bind#2".to_owned(),
            },
            ScriptBind {
                key: "B".to_owned(),
                action: "bind#3".to_owned(),
            },
        ]);
        let hit = km.lookup(KeyChord::parse("<C-g>")?);
        let Some(Action::InvokeScript(slot)) = hit else {
            color_eyre::eyre::bail!("<C-g> 应命中 InvokeScript,实得 {hit:?}");
        };
        assert_eq!(km.script_action(slot), Some("bind#1"));
        let hit = km.lookup(KeyChord::parse("B")?);
        let Some(Action::InvokeScript(slot)) = hit else {
            color_eyre::eyre::bail!("非法键跳过后,B 仍应命中,实得 {hit:?}");
        };
        assert_eq!(km.script_action(slot), Some("bind#3"));
        // 配置槽位不被 bind 追加破坏。
        let hit = km.lookup(KeyChord::parse("X")?);
        let Some(Action::InvokeScript(slot)) = hit else {
            color_eyre::eyre::bail!("配置 keys.script 槽位应保留,实得 {hit:?}");
        };
        assert_eq!(km.script_action(slot), Some("my.first"));
        assert_eq!(km.script_action(ScriptSlot(3)), None, "非法键不占槽位");
        Ok(())
    }

    /// help 目录的 Scripts 组:配置 keys.script 与 daemon bind 追加都进目录,
    /// label = 注册名、排在内建组之后。
    #[test]
    fn help_catalog_lists_script_binds() -> color_eyre::Result<()> {
        use mineral_protocol::ScriptBind;

        use crate::runtime::keymap::help::HelpGroup;
        let dir = tempfile::tempdir()?;
        let user = dir.path().join("config.lua");
        std::fs::write(
            &user,
            "return { tui = { keys = { script = { [\"my.first\"] = \"X\" } } } }",
        )?;
        let (cfg, warnings) = mineral_config::load(&user)?;
        assert!(warnings.is_empty(), "合法配置不应有 warning: {warnings:?}");
        let mut km = Keymap::from_config(cfg.tui().keys(), cfg.tui().behavior());
        km.append_script_binds(&[ScriptBind {
            key: "<C-g>".to_owned(),
            action: "bind#1".to_owned(),
        }]);
        let scripts = km
            .help()
            .iter()
            .filter(|e| *e.group() == HelpGroup::Scripts)
            .map(|e| {
                let chords = e
                    .chords()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<String>>();
                (e.label().to_owned(), chords)
            })
            .collect::<Vec<(String, Vec<String>)>>();
        assert_eq!(
            scripts,
            vec![
                ("my.first".to_owned(), vec!["X".to_owned()]),
                ("bind#1".to_owned(), vec!["<C-g>".to_owned()]),
            ]
        );
        // Scripts 组恒在目录尾部。
        let last_group = km.help().last().map(|e| *e.group());
        assert_eq!(last_group, Some(HelpGroup::Scripts));
        Ok(())
    }
}

//! 键位 cheatsheet 的目录数据([`HelpEntry`])与其构建器。
//!
//! 目录随键表在 [`super::Keymap::from_config`] 一次遍历同源产出:条目键集、
//! label 内嵌的步长实值都与查表落地共用同一份事实,用户重映射后目录自动跟随。

use std::borrow::Cow;

use mineral_config::keys::KeyChord;

/// cheatsheet 分组。渲染顺序 = 声明顺序;`Scripts`(脚本绑定)恒排在内建组之后。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HelpGroup {
    /// 播放控制(暂停 / 切歌 / 音量 / seek)。
    Playback,

    /// 列表与视图内导航(光标移动 / 进退 / 下钻)。
    Navigate,

    /// 对选中实体的操作(love / 下载 / 菜单 / 关通知)。
    Actions,

    /// 布局态与浮层开合(全屏 / 搜索 / 队列 / 退出)。
    View,

    /// 视口滚动(逐行 / 翻页)。
    Scroll,

    /// 脚本具名动作(`keys.script` 与 `mineral.bind`),label = 注册名。
    Scripts,
}

impl HelpGroup {
    /// 组标题(渲染层直接展示)。
    pub fn title(self) -> &'static str {
        match self {
            Self::Playback => "Playback",
            Self::Navigate => "Navigate",
            Self::Actions => "Actions",
            Self::View => "View",
            Self::Scroll => "Scroll",
            Self::Scripts => "Scripts",
        }
    }
}

/// cheatsheet 目录里的一行:一个(或一对合并的)动作的分组、描述与全部绑定键。
#[derive(Clone, Debug)]
pub struct HelpEntry {
    /// 所属分组。
    group: HelpGroup,

    /// 英文短描述(渲染层直接展示;带步长的动作内嵌 behavior 实值,脚本条目 = 注册名)。
    label: Cow<'static, str>,

    /// 全部绑定键,**显示优先序**:合并条目各动作的首键在前、同义余键靠后,
    /// 渲染截前 N 个即得「每个方向各露一键」的效果。
    chords: Vec<KeyChord>,
}

impl HelpEntry {
    /// 所属分组。
    pub fn group(&self) -> &HelpGroup {
        &self.group
    }

    /// 英文短描述。
    pub fn label(&self) -> &str {
        &self.label
    }

    /// 全部绑定键(显示优先序)。
    pub fn chords(&self) -> &[KeyChord] {
        &self.chords
    }
}

/// 目录构建器:按声明序收条目,相邻同(组, label)的动作合并为一行
/// (成对动作如「音量 ±」以同 label 声明即合并)。
#[derive(Default)]
pub(crate) struct CatalogBuilder {
    /// 已收条目(声明序)。
    entries: Vec<HelpEntry>,

    /// 与 `entries` 平行:各条目已合并的动作数,决定下一个合并动作首键的插入位。
    action_counts: Vec<usize>,
}

impl CatalogBuilder {
    /// 收一个动作的绑定。与上一条目同(组, label)则合并:新动作首键插到既有
    /// 各动作首键之后、同义余键追加到尾,保证显示优先序;键集为空(用户解绑)
    /// 则整条跳过,合并对里另一半仍展示。
    ///
    /// # Params:
    ///   - `group`: 所属分组
    ///   - `label`: 描述(成对动作传同一 label 触发合并)
    ///   - `chords`: 该动作的全部绑定键(配置声明序)
    pub(crate) fn push(
        &mut self,
        group: HelpGroup,
        label: impl Into<Cow<'static, str>>,
        chords: &[KeyChord],
    ) {
        let Some((first, rest)) = chords.split_first() else {
            return;
        };
        let label = label.into();
        if let (Some(last), Some(count)) = (self.entries.last_mut(), self.action_counts.last_mut())
            && last.group == group
            && last.label == label
        {
            last.chords.insert(*count, *first);
            last.chords.extend_from_slice(rest);
            *count += 1;
            return;
        }
        self.entries.push(HelpEntry {
            group,
            label,
            chords: chords.to_vec(),
        });
        self.action_counts.push(1);
    }

    /// 收尾,交出目录。
    pub(crate) fn finish(self) -> Vec<HelpEntry> {
        self.entries
    }
}

impl HelpEntry {
    /// 脚本绑定条目([`super::Keymap::append_script_binds`] 运行期追加用;
    /// 内建条目一律经 [`CatalogBuilder`] 产出)。
    pub(crate) fn script(name: &str, chord: KeyChord) -> Self {
        Self {
            group: HelpGroup::Scripts,
            label: Cow::Owned(name.to_owned()),
            chords: vec![chord],
        }
    }
}

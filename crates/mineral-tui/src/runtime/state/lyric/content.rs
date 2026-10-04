//! 当前曲目的歌词内容、标题歌词和可用副歌词显示档。

use mineral_model::{LyricLine, Lyrics};

use super::super::AppState;

/// 歌词面板的副歌词显示档(翻译 / 罗马音),由 `t` 键循环切换。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LyricExtra {
    /// 只显示原文(默认)。
    #[default]
    None,

    /// 原文下叠加行级翻译。
    Translation,

    /// 原文下叠加行级罗马音。
    Romanization,
}

impl LyricExtra {
    /// 稳定字符串名(UI 偏好持久化用),与 [`Self::from_name`] 对偶。
    pub fn name(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Translation => "translation",
            Self::Romanization => "romanization",
        }
    }

    /// 从 [`Self::name`] 的稳定名解析回来。
    ///
    /// # Params:
    ///   - `name`: 稳定名字符串(落库值)
    ///
    /// # Return:
    ///   对应档位;未知名(脏数据)为 `None`,调用方降级到默认档。
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "none" => Some(Self::None),
            "translation" => Some(Self::Translation),
            "romanization" => Some(Self::Romanization),
            _ => None,
        }
    }
}

impl AppState {
    /// 当前曲目的完整歌词集合;未拉到时返回 `None`。
    fn current_lyrics_set(&self) -> Option<&Lyrics> {
        let song = self.playback.track.as_ref()?;
        self.library.lyrics.get(&song.id)
    }

    /// 当前曲目的歌词行序列(行级 / 逐字 / 有时间 / 无时间混排,翻译 / 罗马音已内嵌在
    /// 各行上);未拉到时返回 `None`。
    pub fn current_lines(&self) -> Option<&[LyricLine]> {
        self.current_lyrics_set().map(|l| l.lines.as_slice())
    }

    /// 当前正在唱的歌词行文本,供窗口标题用。时间轴失真档(顶换流时长对不上)返回
    /// `None`——与歌词面板同口径,不显示错行;无同步 / 无当前行同样 `None`。
    /// 逐字行文本按需拼接故返回拥有串。
    pub(crate) fn active_title_lyric(&self) -> Option<String> {
        if self.playback.sync_trust() == crate::runtime::playback::SyncTrust::Broken {
            return None;
        }
        let lines = self.current_lines()?;
        let idx = mineral_model::current_line(lines, self.playback.position_ms)?;
        lines.get(idx).map(|line| line.kind.text().into_owned())
    }

    /// 把副歌词切换意图交给当前歌词面板。
    pub fn cycle_lyric_extra(&mut self) {
        let input = crate::view::lyrics::input(&self.playback, &self.library);
        self.browse
            .lyrics
            .cycle_extra(input, self.cfg.tui().animation());
    }
}

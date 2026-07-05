//! TUI 窗口标题段配置。默认值全部在 `default.lua`（deep_merge 恒补全），本文件不写。

use mineral_config_macros::config_section;
use serde::Deserialize;

/// TUI 窗口标题配置。
///
/// 字段私有 + `#[non_exhaustive]`，经 getter 读取。默认值以 `default.lua` 为准。
#[config_section]
pub struct WindowTitleConfig {
    /// 总开关。`false` 时不 push/pop 标题栈，也不发送任何 OSC 标题序列。
    enabled: bool,

    /// 四态状态图标字形（`StateIcon` 段按当前态解析）。
    icons: TitleIcons,

    /// 有歌态（播放 / 暂停共用）模板。
    template: Vec<TitleSegment>,

    /// 空闲态（无当前歌）模板。
    idle: Vec<TitleSegment>,

    /// 断连态（失联 daemon）模板。
    disconnected: Vec<TitleSegment>,
}

/// 四态状态图标字形。默认符号在 `default.lua`；用户部分覆盖经 deep_merge 补全其余。
#[derive(Clone, Debug, Deserialize, derive_getters::Getters)]
pub struct TitleIcons {
    /// 播放中图标（动作图标惯例：按下即暂停）。
    playing: String,

    /// 暂停图标（按下即播放）。
    paused: String,

    /// 空闲图标。
    idle: String,

    /// 断连图标。
    disconnected: String,
}

/// 窗口标题模板的一个段。
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum TitleSegment {
    /// 当前态的状态图标；字形取自 [`TitleIcons`] 按当前态解析。
    StateIcon {
        /// 必须为 `true`；`false` 会被视为无法识别的段并报错。
        icon: bool,
    },

    /// 取自当前上下文的一个字段，空值时整段（含 prefix/suffix）不输出。
    Field {
        /// 要引用的字段。
        field: TitleField,

        /// 字段值前的固定文本（省略 = 空）。
        #[serde(default)]
        prefix: String,

        /// 字段值后的固定文本（省略 = 空）。
        #[serde(default)]
        suffix: String,

        /// 时间字段（`Position` / `Duration`）的渲染格式；非时间字段忽略。
        /// 省略即 [`TimeFormat::Preset(TimePreset::Clock)`]——这是段内可选属性的类型语义，
        /// 非配置默认（用户数组内逐段可选，`default.lua` 填不进来）。
        #[serde(default)]
        format: TimeFormat,
    },

    /// 字面文本。
    Literal {
        /// 要输出的固定字符串。
        text: String,
    },
}

/// 模板可引用的字段。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TitleField {
    /// 歌名（`Song.name`，必有）。
    Title,

    /// 首个艺人的名字；无艺人时整段折叠。
    Artist,

    /// 专辑名；单曲为 `None` 时整段折叠。
    Album,

    /// 当前播放进度；按 `format` 渲染，恒输出（0 → `00:00`）。
    Position,

    /// 当前曲目全长；按 `format` 渲染，为 0（未探出）时整段折叠。
    Duration,

    /// 来源标签（`Song.source().label()`，如 netease / bilibili）。
    Source,

    /// 当前正在唱的歌词行；无同步 / 时间轴失真 / 无当前行时整段折叠。
    Lyric,
}

/// 时间字段的渲染格式。
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum TimeFormat {
    /// 预设：`"clock"`（mm:ss，>=1h 自动 h:mm:ss）/ `"seconds"`（总秒数）。
    Preset(TimePreset),

    /// 自定义占位串：`{ pattern = "{m}:{ss}" }`，占位符 `{h}{hh}{m}{mm}{s}{ss}`（最细到秒）。
    Pattern {
        /// 占位串。
        pattern: String,
    },
}

impl Default for TimeFormat {
    fn default() -> Self {
        Self::Preset(TimePreset::Clock)
    }
}

impl TimeFormat {
    /// 把毫秒渲染成字符串。
    ///
    /// # Params:
    ///   - `ms`: 毫秒时长 / 进度。
    ///
    /// # Return:
    ///   格式化后的时间字符串。
    pub fn render(&self, ms: u64) -> String {
        match self {
            Self::Preset(TimePreset::Seconds) => (ms / 1000).to_string(),
            Self::Preset(TimePreset::Clock) => {
                let total_s = ms / 1000;
                let (h, m, s) = (total_s / 3600, (total_s / 60) % 60, total_s % 60);
                if h > 0 {
                    format!("{h}:{m:02}:{s:02}")
                } else {
                    format!("{m:02}:{s:02}")
                }
            }
            Self::Pattern { pattern } => render_pattern(pattern, ms),
        }
    }
}

/// 时间预设格式。
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TimePreset {
    /// mm:ss，>=1h 自动进 h:mm:ss。
    Clock,

    /// 总秒数。
    Seconds,
}

/// 按占位串渲染毫秒。分 = 时内余（0–59），秒 = 分内余（0–59）；双写补零。
/// 先替长 token（`{hh}` 前于 `{h}`）避免部分匹配。
fn render_pattern(pattern: &str, ms: u64) -> String {
    let h = ms / 3_600_000;
    let m = (ms / 60_000) % 60;
    let s = (ms / 1000) % 60;
    pattern
        .replace("{hh}", &format!("{h:02}"))
        .replace("{mm}", &format!("{m:02}"))
        .replace("{ss}", &format!("{s:02}"))
        .replace("{h}", &h.to_string())
        .replace("{m}", &m.to_string())
        .replace("{s}", &s.to_string())
}

#[cfg(test)]
mod tests {
    use super::{TimeFormat, TimePreset};

    /// Clock 预设：mm:ss，>=1h 进 h:mm:ss。
    #[test]
    fn clock_format() {
        let clock = TimeFormat::Preset(TimePreset::Clock);
        assert_eq!(clock.render(0), "00:00");
        assert_eq!(clock.render(83_000), "01:23");
        assert_eq!(clock.render(3_723_000), "1:02:03");
    }

    /// Seconds 预设：总秒数。
    #[test]
    fn seconds_format() {
        assert_eq!(TimeFormat::Preset(TimePreset::Seconds).render(83_000), "83");
    }

    /// Pattern：占位串按分内余 / 秒内余替换，双写补零。
    #[test]
    fn pattern_format() {
        let p = TimeFormat::Pattern {
            pattern: "{m}:{ss}".to_owned(),
        };
        assert_eq!(p.render(83_000), "1:23");
        let hms = TimeFormat::Pattern {
            pattern: "{h}h{mm}m{ss}s".to_owned(),
        };
        assert_eq!(hms.render(3_723_000), "1h02m03s");
    }
}

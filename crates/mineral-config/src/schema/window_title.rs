//! TUI 窗口标题段配置。默认值全部在 `default.lua`（deep_merge 恒补全），本文件不写。

use mineral_config_macros::{config_section, lua_enum};
use serde::Deserialize;

/// 窗口标题
#[config_section]
pub struct WindowTitleConfig {
    /// 是否启用
    enabled: bool,

    /// 状态图标
    icons: TitleIcons,

    /// 播放及暂停时的模板
    template: Vec<TitleSegment>,

    /// 无歌曲时的模板
    idle: Vec<TitleSegment>,

    /// 断连时的模板
    disconnected: Vec<TitleSegment>,
}

/// 状态图标
#[config_section]
pub struct TitleIcons {
    /// 播放中图标
    playing: String,

    /// 暂停图标
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
    /// 当前状态图标
    StateIcon {
        /// 是否显示状态图标
        icon: bool,
    },

    /// 上下文字段，空值时连同前后缀隐藏
    Field {
        /// 要引用的字段。
        field: TitleField,

        /// 字段前缀
        #[serde(default)]
        prefix: String,

        /// 字段后缀
        #[serde(default)]
        suffix: String,

        /// 时间格式；省略用 clock，非时间字段忽略
        #[serde(default)]
        format: TimeFormat,
    },

    /// 字面文本。
    Literal {
        /// 固定文本
        text: String,
    },
}

/// 标题可引用字段
#[lua_enum]
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TitleField {
    /// 歌名
    Title,

    /// 首个艺人名
    Artist,

    /// 专辑名
    Album,

    /// 播放进度
    Position,

    /// 曲目时长；未知时隐藏
    Duration,

    /// 来源名
    Source,

    /// 当前歌词行
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

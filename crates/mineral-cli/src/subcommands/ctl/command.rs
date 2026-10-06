//! `mineral ctl` 的命令形状与参数解析。
//!
//! 绝对取值在这里落成毫秒 / 百分比;相对形式只解析成偏移,由调用方按当前镜像解析成目标值。

use std::str::FromStr;

use clap::{Args, Subcommand, ValueEnum};
use mineral_protocol::PlayMode;

/// 控制命令的位置和音量参数解析失败。
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    /// 歌单身份必须包含来源和来源内 ID。
    #[error("playlist id must have the form source:id")]
    PlaylistId,

    /// 时间格式不符合可接受的段数。
    #[error("时长 `{raw}` 用法:SS / MM:SS / H:MM:SS 或 <n>s|m|h")]
    TimeFormat {
        /// 原始参数。
        raw: String,
    },

    /// 时间段不是无符号整数。
    #[error("时长 `{raw}` 含非数字段 `{field}`")]
    TimeField {
        /// 原始参数。
        raw: String,

        /// 失败的时间段。
        field: String,

        /// 整数解析错误。
        #[source]
        source: std::num::ParseIntError,
    },

    /// 非最高位的分秒段超过 59。
    #[error("时长 `{raw}` 的 `{field}` 超出 0..=59")]
    TimeRange {
        /// 原始参数。
        raw: String,

        /// 越界的段。
        field: String,
    },

    /// 时间量无法换算为毫秒。
    #[error("时长 `{raw}` 过大")]
    TimeOverflow {
        /// 原始参数。
        raw: String,
    },

    /// 相对位置解析失败。
    #[error("相对位置 `{raw}` 不合法")]
    RelativeTime {
        /// 原始带符号参数。
        raw: String,

        /// 时间量错误。
        #[source]
        source: Box<Self>,
    },

    /// 相对位置超过有符号整数上限。
    #[error("相对位置 `{raw}` 过大")]
    RelativeOverflow {
        /// 原始参数。
        raw: String,

        /// 整数转换错误。
        #[source]
        source: std::num::TryFromIntError,
    },

    /// 音量增量不是有符号整数。
    #[error("音量 `{raw}` 的增量不是 -32768..=32767 的整数")]
    VolumeDelta {
        /// 原始参数。
        raw: String,

        /// 整数解析错误。
        #[source]
        source: std::num::ParseIntError,
    },

    /// 绝对音量不是无符号整数。
    #[error("音量 `{raw}` 不是 0..=100 的整数")]
    VolumeNumber {
        /// 原始参数。
        raw: String,

        /// 整数解析错误。
        #[source]
        source: std::num::ParseIntError,
    },

    /// 绝对音量越界。
    #[error("音量 `{raw}` 超出 0..=100")]
    VolumeRange {
        /// 原始参数。
        raw: String,
    },
}

/// `mineral ctl` 的参数。
#[derive(Args, Debug, Clone)]
pub struct CtlArgs {
    /// print the conclusion as a single JSON line
    #[arg(long)]
    pub json: bool,

    /// control command
    #[command(subcommand)]
    pub cmd: CtlCommand,
}

/// daemon 控制命令(都不需要歌曲身份)。
#[derive(Subcommand, Debug, Clone)]
pub enum CtlCommand {
    /// manage saved Mineral playlists
    Playlist {
        /// playlist subcommand
        #[command(subcommand)]
        cmd: PlaylistCommand,
    },

    /// toggle between pause and resume (skipped when there is no current track)
    PlayPause,

    /// pause playback
    Pause,

    /// resume playback
    Resume,

    /// stop the current track
    Stop,

    /// advance to the next track
    Next,

    /// go to the previous track; past the restart threshold it seeks to the start
    Prev,

    /// show or set the playback mode
    Mode {
        /// target mode; omit to cycle to the next one
        #[arg(value_enum)]
        mode: Option<ModeArg>,
    },

    /// seek to a position (SS, MM:SS, H:MM:SS or <n>s|m|h; `+`/`-` prefix for relative)
    Seek {
        /// target position
        #[arg(allow_hyphen_values = true)]
        target: SeekSpec,
    },

    /// set the volume (0..=100; `+`/`-` prefix for a delta)
    Volume {
        /// target volume percentage
        #[arg(allow_hyphen_values = true)]
        target: VolumeSpec,
    },

    /// queue structure edits
    Queue {
        /// queue subcommand
        #[command(subcommand)]
        cmd: QueueCommand,
    },
}

impl CtlCommand {
    /// JSON `command` 字段值(子命令路径)。
    pub(super) fn path(&self) -> &'static str {
        match self {
            Self::PlayPause => "play-pause",
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Stop => "stop",
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Mode { .. } => "mode",
            Self::Seek { .. } => "seek",
            Self::Volume { .. } => "volume",
            Self::Queue { cmd } => cmd.path(),
            Self::Playlist { cmd } => cmd.path(),
        }
    }
}

/// `ctl queue` 下的编辑命令。
#[derive(Subcommand, Debug, Clone)]
pub enum QueueCommand {
    /// save the entire queue as a new Mineral playlist
    Save {
        /// name of the new playlist
        name: String,
    },

    /// apply a named transform registered in `queue.transforms`
    Transform {
        /// stable transform name advertised by the daemon
        name: String,

        /// 0-based queue index the transform treats as the cursor
        #[arg(long)]
        at: Option<usize>,
    },

    /// undo the last queue edit
    Undo,
}

impl QueueCommand {
    /// JSON `command` 字段值(子命令路径)。
    pub(super) fn path(&self) -> &'static str {
        match self {
            Self::Save { .. } => "queue save",
            Self::Transform { .. } => "queue transform",
            Self::Undo => "queue undo",
        }
    }
}

/// `mode` 参数的档位取值。
#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum ModeArg {
    /// play through, then stop
    Sequential,

    /// shuffle once, then play through
    Shuffle,

    /// repeat the whole queue
    RepeatAll,

    /// repeat one track
    RepeatOne,
}

impl ModeArg {
    /// 人读档位名(与命令行取值同名)。
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Sequential => "sequential",
            Self::Shuffle => "shuffle",
            Self::RepeatAll => "repeat-all",
            Self::RepeatOne => "repeat-one",
        }
    }
}

impl From<ModeArg> for PlayMode {
    fn from(value: ModeArg) -> Self {
        match value {
            ModeArg::Sequential => Self::Sequential,
            ModeArg::Shuffle => Self::Shuffle,
            ModeArg::RepeatAll => Self::RepeatAll,
            ModeArg::RepeatOne => Self::RepeatOne,
        }
    }
}

/// `seek` 的取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekSpec {
    /// 绝对位置(毫秒)。
    Absolute(u64),

    /// 相对当前位置的偏移(毫秒,负数为回退)。
    Relative(i64),
}

/// `volume` 的取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeSpec {
    /// 绝对百分比(0..=100)。
    Absolute(u8),

    /// 相对当前音量的增减(百分点)。
    Relative(i16),
}

/// 毫秒 → 人读位置(`mm:ss`,满一小时带小时段)。
///
/// # Params:
///   - `ms`: 毫秒数
pub(super) fn format_position(ms: u64) -> String {
    let total = ms / 1_000;
    let (hours, minutes, seconds) = (total / 3_600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

impl FromStr for SeekSpec {
    type Err = ParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let raw = raw.trim();
        if let Some(rest) = raw.strip_prefix('+') {
            return magnitude_ms(rest, raw).map(Self::Relative);
        }
        if let Some(rest) = raw.strip_prefix('-') {
            return magnitude_ms(rest, raw).map(|magnitude| Self::Relative(-magnitude));
        }
        parse_time_ms(raw).map(Self::Absolute)
    }
}

/// 相对偏移的绝对值(毫秒);负号由调用方处理。
///
/// # Params:
///   - `rest`: 去掉符号后的取值
///   - `raw`: 原始取值(报错时原样回显)
fn magnitude_ms(rest: &str, raw: &str) -> Result<i64, ParseError> {
    let ms = parse_time_ms(rest).map_err(|source| ParseError::RelativeTime {
        raw: raw.to_owned(),
        source: Box::new(source),
    })?;
    i64::try_from(ms).map_err(|source| ParseError::RelativeOverflow {
        raw: raw.to_owned(),
        source,
    })
}

impl FromStr for VolumeSpec {
    type Err = ParseError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let raw = raw.trim();
        if let Some(rest) = raw.strip_prefix('+') {
            return volume_delta(rest, raw).map(Self::Relative);
        }
        if let Some(rest) = raw.strip_prefix('-') {
            return volume_delta(rest, raw).map(|delta| Self::Relative(-delta));
        }
        parse_volume_absolute(raw).map(Self::Absolute)
    }
}

/// 相对音量的增量(百分点);负号由调用方处理。
///
/// # Params:
///   - `rest`: 去掉符号后的取值
///   - `raw`: 原始取值(报错时原样回显)
fn volume_delta(rest: &str, raw: &str) -> Result<i16, ParseError> {
    rest.trim()
        .parse::<i16>()
        .map_err(|source| ParseError::VolumeDelta {
            raw: raw.to_owned(),
            source,
        })
}

/// 解析时间量:`SS`、`MM:SS`、`H:MM:SS` 或 `<n>s|m|h`,都落到毫秒。
///
/// # Params:
///   - `raw`: 命令行原始取值
///
/// # Return:
///   毫秒;写法不合法时返回结构化解析错误(clap 负责展示)。
fn parse_time_ms(raw: &str) -> Result<u64, ParseError> {
    let raw = raw.trim();
    if let Some((digits, unit)) = split_unit_suffix(raw) {
        let value = digits
            .parse::<u64>()
            .map_err(|source| ParseError::TimeField {
                raw: raw.to_owned(),
                field: digits.to_owned(),
                source,
            })?;
        let seconds_per_unit = match unit {
            's' => 1_u64,
            'm' => 60,
            _ => 3_600,
        };
        return value
            .checked_mul(seconds_per_unit)
            .and_then(|seconds| seconds.checked_mul(1_000))
            .ok_or_else(|| ParseError::TimeOverflow {
                raw: raw.to_owned(),
            });
    }
    parse_colon_ms(raw)
}

/// 取「数字 + 单位后缀」形式;不是该形式返回 `None`。
///
/// # Params:
///   - `raw`: 命令行原始取值
fn split_unit_suffix(raw: &str) -> Option<(&str, char)> {
    let mut chars = raw.chars();
    let unit = chars.next_back()?;
    if !matches!(unit, 's' | 'm' | 'h') {
        return None;
    }
    let digits = chars.as_str();
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    Some((digits, unit))
}

/// 解析 `SS` / `MM:SS` / `H:MM:SS` 为毫秒。
///
/// 最高位不设上限(允许 `90:00` 表示 90 分钟);其余位必须在 0..=59 内,`1:99` 这类写法按输入
/// 错误拒绝,不做进位猜测。
///
/// # Params:
///   - `raw`: 命令行原始取值
fn parse_colon_ms(raw: &str) -> Result<u64, ParseError> {
    let fields = raw.split(':').collect::<Vec<&str>>();
    if fields.is_empty() || fields.len() > 3 {
        return Err(ParseError::TimeFormat {
            raw: raw.to_owned(),
        });
    }
    let mut total_seconds = 0_u64;
    for (index, field) in fields.iter().enumerate() {
        let value = field
            .parse::<u64>()
            .map_err(|source| ParseError::TimeField {
                raw: raw.to_owned(),
                field: (*field).to_owned(),
                source,
            })?;
        // 最高位(冒号段的首位)不设上限,其余位是分 / 秒,必须合法。
        let leading = index == 0;
        if !leading && value > 59 {
            return Err(ParseError::TimeRange {
                raw: raw.to_owned(),
                field: (*field).to_owned(),
            });
        }
        total_seconds = total_seconds
            .checked_mul(60)
            .and_then(|sum| sum.checked_add(value))
            .ok_or_else(|| ParseError::TimeOverflow {
                raw: raw.to_owned(),
            })?;
    }
    total_seconds
        .checked_mul(1_000)
        .ok_or_else(|| ParseError::TimeOverflow {
            raw: raw.to_owned(),
        })
}

/// 解析绝对音量(0..=100)。
///
/// # Params:
///   - `raw`: 命令行原始取值
fn parse_volume_absolute(raw: &str) -> Result<u8, ParseError> {
    let value = raw
        .trim()
        .parse::<u16>()
        .map_err(|source| ParseError::VolumeNumber {
            raw: raw.to_owned(),
            source,
        })?;
    u8::try_from(value)
        .ok()
        .filter(|pct| *pct <= 100)
        .ok_or_else(|| ParseError::VolumeRange {
            raw: raw.to_owned(),
        })
}

#[cfg(test)]
mod tests {
    use super::{ModeArg, QueueCommand, SeekSpec, VolumeSpec};
    use mineral_protocol::PlayMode;

    /// 解析一个位置;写法不合法即测试失败。
    fn seek(raw: &str) -> color_eyre::Result<SeekSpec> {
        raw.parse::<SeekSpec>().map_err(Into::into)
    }

    /// 解析一个音量;写法不合法即测试失败。
    fn volume(raw: &str) -> color_eyre::Result<VolumeSpec> {
        raw.parse::<VolumeSpec>().map_err(Into::into)
    }

    /// 绝对位置的各种写法都落到毫秒,单位后缀与冒号段等价。
    #[test]
    fn absolute_seek_forms() -> color_eyre::Result<()> {
        assert_eq!(seek("90")?, SeekSpec::Absolute(90_000), "裸秒");
        assert_eq!(seek("1:30")?, SeekSpec::Absolute(90_000), "分:秒");
        assert_eq!(seek("1:02:03")?, SeekSpec::Absolute(3_723_000), "时:分:秒");
        assert_eq!(
            seek("90:00")?,
            SeekSpec::Absolute(5_400_000),
            "最高位可超 59"
        );
        assert_eq!(seek("10s")?, SeekSpec::Absolute(10_000), "单位秒");
        assert_eq!(seek("2m")?, SeekSpec::Absolute(120_000), "单位分");
        assert_eq!(seek("1h")?, SeekSpec::Absolute(3_600_000), "单位时");
        Ok(())
    }

    /// 带符号即相对,符号后的量按同一套写法解析。
    #[test]
    fn relative_seek_forms() -> color_eyre::Result<()> {
        assert_eq!(seek("+10s")?, SeekSpec::Relative(10_000));
        assert_eq!(seek("-30")?, SeekSpec::Relative(-30_000), "裸秒带符号");
        assert_eq!(seek("+1:30")?, SeekSpec::Relative(90_000), "分:秒");
        assert_eq!(seek("-2m")?, SeekSpec::Relative(-120_000));
        assert!(seek("+1:99").is_err(), "秒位越界");
        assert!(seek("10+").is_err(), "符号不在开头");
        Ok(())
    }

    /// 非法位置按输入错误拒绝,不静默取近似值。
    #[test]
    fn seek_rejects_garbage() {
        assert!("1:xx".parse::<SeekSpec>().is_err(), "非数字段");
        assert!("1:2:3:4".parse::<SeekSpec>().is_err(), "段数过多");
        assert!("".parse::<SeekSpec>().is_err(), "空串");
        assert!("1:".parse::<SeekSpec>().is_err(), "空段");
        assert!("abc".parse::<SeekSpec>().is_err(), "非数字");
    }

    /// 绝对音量限 0..=100;带符号是相对增量。
    #[test]
    fn volume_forms() -> color_eyre::Result<()> {
        assert_eq!(volume("0")?, VolumeSpec::Absolute(0));
        assert_eq!(volume("100")?, VolumeSpec::Absolute(100));
        assert_eq!(volume(" 45 ")?, VolumeSpec::Absolute(45), "两侧空白容忍");
        assert_eq!(volume("+5")?, VolumeSpec::Relative(5));
        assert_eq!(volume("-5")?, VolumeSpec::Relative(-5), "负号即相对");
        assert_eq!(volume("-1")?, VolumeSpec::Relative(-1));
        assert!(volume("101").is_err(), "越界");
        assert!(volume("+x").is_err(), "增量非数字");
        Ok(())
    }

    /// 档位名与协议取值一一对应。
    #[test]
    fn mode_args_map_to_protocol() {
        assert_eq!(PlayMode::from(ModeArg::Sequential), PlayMode::Sequential);
        assert_eq!(PlayMode::from(ModeArg::Shuffle), PlayMode::Shuffle);
        assert_eq!(PlayMode::from(ModeArg::RepeatAll), PlayMode::RepeatAll);
        assert_eq!(PlayMode::from(ModeArg::RepeatOne), PlayMode::RepeatOne);
        assert_eq!(ModeArg::Shuffle.label(), "shuffle");
    }

    /// 子命令路径是 JSON 契约的一部分。
    #[test]
    fn queue_paths_are_stable() {
        assert_eq!(QueueCommand::Undo.path(), "queue undo");
        assert_eq!(
            QueueCommand::Transform {
                name: "dedupe".to_owned(),
                at: None,
            }
            .path(),
            "queue transform"
        );
    }
}

/// 自建歌单管理命令，按完整来源身份定位。
#[derive(Subcommand, Debug, Clone)]
pub enum PlaylistCommand {
    /// rename a saved playlist
    Rename {
        /// qualified playlist id (source:id)
        #[arg(value_parser = parse_playlist_id)]
        id: mineral_model::PlaylistId,

        /// new playlist name
        name: String,
    },

    /// delete a saved playlist, keeping songs and playback intact
    Delete {
        /// qualified playlist id (source:id)
        #[arg(value_parser = parse_playlist_id)]
        id: mineral_model::PlaylistId,
    },
}

impl PlaylistCommand {
    /// JSON 信封中的命令路径。
    pub(super) fn path(&self) -> &'static str {
        match self {
            Self::Rename { .. } => "playlist rename",
            Self::Delete { .. } => "playlist delete",
        }
    }
}

/// CLI 接受完整身份，避免同名歌单定位歧义。
fn parse_playlist_id(raw: &str) -> Result<mineral_model::PlaylistId, ParseError> {
    let (source, value) = raw
        .split_once(':')
        .filter(|(source, value)| !source.is_empty() && !value.is_empty())
        .ok_or(ParseError::PlaylistId)?;
    Ok(mineral_model::PlaylistId::new(
        mineral_model::SourceKind::from_name(source),
        value,
    ))
}

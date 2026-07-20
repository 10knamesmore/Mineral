//! 播放循环模式 + Server 端持有的播放状态快照。
//!
//! `PlayMode` 历史在 mineral-tui::playback;搬来这里因为 server 端也要决定
//! `next_song` / `prev_song`,且需要走 wire([`PlayerSync`] 的字段)。
//! glyph / label 这两个 UI 字面量跟着挪过来 —— 字符画放 protocol 不优雅,
//! 但避免 mineral-tui 跨 crate 加 inherent impl 的麻烦,**够用**。

use mineral_model::{Envelope, PlayUrl, Song, SongId};
use serde::{Deserialize, Serialize};

/// 播放循环模式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayMode {
    /// 顺序播放(到底停止)。
    #[default]
    Sequential,
    /// 随机播放(进 Shuffle 时洗一次 queue,之后顺序推)。
    Shuffle,
    /// 整列循环。
    RepeatAll,
    /// 单曲循环。
    RepeatOne,
}

impl PlayMode {
    /// `m` 键循环到下一档。
    #[must_use]
    pub fn cycle(self) -> Self {
        match self {
            Self::Sequential => Self::Shuffle,
            Self::Shuffle => Self::RepeatAll,
            Self::RepeatAll => Self::RepeatOne,
            Self::RepeatOne => Self::Sequential,
        }
    }

    /// transport 模式按钮字形。
    #[must_use]
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Sequential => "→",
            Self::Shuffle => "⇄",
            Self::RepeatAll => "↻∞",
            Self::RepeatOne => "↻¹",
        }
    }

    /// vol/mode/sort 行短标签。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Sequential => "seq",
            Self::Shuffle => "shuffle",
            Self::RepeatAll => "repeat-all",
            Self::RepeatOne => "repeat-one",
        }
    }

    /// 稳定字符串名(会话持久化用),与 [`Self::from_name`] 对偶。
    ///
    /// 值与历史上落库的 Debug 名一致(`"Sequential"` 等),存量会话库无缝。
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Sequential => "Sequential",
            Self::Shuffle => "Shuffle",
            Self::RepeatAll => "RepeatAll",
            Self::RepeatOne => "RepeatOne",
        }
    }

    /// 从 [`Self::name`] 的稳定名解析回来。
    ///
    /// # Params:
    ///   - `name`: 稳定名字符串(落库值)
    ///
    /// # Return:
    ///   对应档位;未知名(脏数据 / 未来新档回退旧版)为 `None`,调用方自行降级。
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "Sequential" => Some(Self::Sequential),
            "Shuffle" => Some(Self::Shuffle),
            "RepeatAll" => Some(Self::RepeatAll),
            "RepeatOne" => Some(Self::RepeatOne),
            _ => None,
        }
    }

    /// 脚本边界的蛇形稳定名(Lua 生态惯例),与 [`Self::from_script_name`] 对偶。
    ///
    /// 与 [`Self::name`](落库格式,变体名形式)是**两套**字符串:落库的不能动
    /// (存量会话库),脚本面(`mineral.player.set_mode` / 属性树 `player.mode`
    /// 的值)统一用这套蛇形。
    #[must_use]
    pub fn script_name(self) -> &'static str {
        match self {
            Self::Sequential => "sequential",
            Self::Shuffle => "shuffle",
            Self::RepeatAll => "repeat_all",
            Self::RepeatOne => "repeat_one",
        }
    }

    /// 从 [`Self::script_name`] 的蛇形名解析回来。
    ///
    /// # Params:
    ///   - `name`: 蛇形名字符串(脚本侧输入)
    ///
    /// # Return:
    ///   对应档位;未知名为 `None`,调用方报脚本错误。
    #[must_use]
    pub fn from_script_name(name: &str) -> Option<Self> {
        match name {
            "sequential" => Some(Self::Sequential),
            "shuffle" => Some(Self::Shuffle),
            "repeat_all" => Some(Self::RepeatAll),
            "repeat_one" => Some(Self::RepeatOne),
            _ => None,
        }
    }

    /// 是否随机播放(queue 被洗过)。MPRIS `Shuffle` 维度。
    #[must_use]
    pub fn shuffle(self) -> bool {
        matches!(self, Self::Shuffle)
    }

    /// 循环维度。MPRIS `LoopStatus` 维度。随机本就一直循环列表,故按 `All` 算。
    #[must_use]
    pub fn repeat(self) -> Repeat {
        match self {
            Self::Sequential => Repeat::Off,
            Self::Shuffle | Self::RepeatAll => Repeat::All,
            Self::RepeatOne => Repeat::One,
        }
    }

    /// 改写「随机」维度、保「循环」维度不变,塌缩回四档之一。
    #[must_use]
    pub fn with_shuffle(self, shuffle: bool) -> Self {
        Self::from_dimensions(shuffle, self.repeat())
    }

    /// 改写「循环」维度、保「随机」维度不变,塌缩回四档之一。
    #[must_use]
    pub fn with_repeat(self, repeat: Repeat) -> Self {
        Self::from_dimensions(self.shuffle(), repeat)
    }

    /// (随机, 循环) 两维度塌缩回四档。
    ///
    /// mineral 只有四档,表达不了「随机 + 循环」同开:随机开时,整列循环被吸收进
    /// `Shuffle`(随机本就一直循环),只有「随机 + 单曲循环」落到 `RepeatOne`。
    fn from_dimensions(shuffle: bool, repeat: Repeat) -> Self {
        match (shuffle, repeat) {
            (false, Repeat::Off) => Self::Sequential,
            (false, Repeat::All) => Self::RepeatAll,
            (false, Repeat::One) => Self::RepeatOne,
            (true, Repeat::One) => Self::RepeatOne,
            (true, Repeat::Off | Repeat::All) => Self::Shuffle,
        }
    }
}

/// 循环维度,独立于「随机」维度;对应 MPRIS `LoopStatus`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Repeat {
    /// 不循环(列表放完即止)。
    #[default]
    Off,

    /// 单曲循环。
    One,

    /// 整列循环。
    All,
}

/// 当前在播音频的来源。transport 据此显徽标;`None` = 未知(从未播 / 重连初帧)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackOrigin {
    /// 下载导出库(永久,文件系统即真相)。
    Download,

    /// 音频本体缓存(LRU,可被淘汰)。
    Cache,

    /// 远端流(可能边播边 capture 入缓存)。
    Remote,
}

/// 当前歌在队列中的位置——server 的 prev/next 锚点,不是 UI 光标。
///
/// 推进以**下标**为真相而非歌曲身份:队列含重复曲时,按身份 first-match 定位会把位置
/// 吸附到首个副本,两首交替的重复曲会互相指回对方,造成无限循环跳不出去。
///
/// [`Self::Detached`] 表达「当前曲已被摘出队列但仍在出声」——把在播曲从队列删掉时,
/// 声音不打断(音频引擎不感知队列),但它在队列里已无下标可指。裸 `usize` 表达不了这个
/// 状态:留在原下标会让推进逻辑再 +1、白白跳过一首。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlayCursor {
    /// 当前曲在队列中,值为其下标。
    InQueue(usize),

    /// 当前曲已被摘出队列但仍在出声。
    Detached {
        /// 当前曲播完后应接的下标;等于队列长度表示播完即停。
        resume_at: usize,
    },
}

impl Default for PlayCursor {
    fn default() -> Self {
        Self::InQueue(0)
    }
}

impl PlayCursor {
    /// 推进 / 后退计算的基准下标。
    ///
    /// # Return:
    ///   队列内时为当前曲下标;悬空时为接续点(当前曲已不占下标)。
    #[inline]
    pub fn anchor(self) -> usize {
        match self {
            Self::InQueue(index) | Self::Detached { resume_at: index } => index,
        }
    }

    /// 当前曲是否仍在队列中。
    #[inline]
    pub fn is_attached(self) -> bool {
        matches!(self, Self::InQueue(_))
    }

    /// 当前曲在队列中的下标;悬空时为 `None`(它已不在队列里)。
    ///
    /// 展示层据此决定给哪一行画在播标记——悬空时不该有任何一行被标记。
    #[inline]
    pub fn queue_index(self) -> Option<usize> {
        match self {
            Self::InQueue(index) => Some(index),
            Self::Detached { .. } => None,
        }
    }
}

/// Client 已持有的播放状态版本号,随 [`crate::Request::PlayerSync`] 上报。
///
/// `0` = 一无所有(启动初次同步);server 端版本从 1 起步、per-process 单调递增,
/// 故 0 必然不匹配、必然换回全量重段。版本号只在单条连接内有意义(断链即退出,
/// 无跨连接陈旧版本问题)。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct PlayerVersions {
    /// queue + original_queue 的版本。
    pub queue: u64,

    /// current_song / play_url / lyrics 的版本。
    pub current: u64,
}

/// 每 tick 的播放状态同步应答:轻段恒有(<100B),重段仅 client 版本落后时附带。
///
/// 重段缺席(`None`)语义是「与你已有的一致」,**不是清空** —— client 必须保持
/// 上次应用的镜像不动。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlayerSync {
    /// server 当前版本;client 收下后于下次请求回报。
    pub versions: PlayerVersions,

    /// queue 中「当前歌」的位置。轻段。
    pub cursor: PlayCursor,

    /// 当前播放模式。轻段。
    pub play_mode: PlayMode,

    /// 当前在播音频的来源(下载 / 缓存 / 远端);`None` = 未知。轻段。
    pub play_origin: Option<PlaybackOrigin>,

    /// client 的 queue 版本落后时才 `Some`。
    pub queue: Option<QueueSync>,

    /// client 的 current 版本落后时才 `Some`。
    pub current: Option<CurrentSync>,
}

/// [`PlayerSync`] 的 queue 重段:整队列 + shuffle 原序,随 `queue` 版本整体更替。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct QueueSync {
    /// 当前 queue 列表(顺序模式 = 原序;shuffle 模式 = 洗过)。
    pub queue: Vec<Song>,

    /// Shuffle 进入时保存的原序;非 Shuffle 状态恒 `None`。
    pub original_queue: Option<Vec<Song>>,
}

/// [`PlayerSync`] 的 current 重段:当前歌上下文,随 `current` 版本整体更替。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct CurrentSync {
    /// 当前在播的歌(`None` 表示从未播过 / 已 stop)。
    pub current_song: Option<Song>,

    /// 当前歌的播放 URL 元信息(format / bitrate);transport 用。
    pub play_url: Option<PlayUrl>,

    /// 当前歌的歌词原文(server 端缓存最新一首)。client 拿来解析成行。
    pub current_lyrics: Option<mineral_model::Lyrics>,

    /// 当前歌对应的 song_id,用于 client 端校验 lyrics 是否跟得上 current_song。
    pub current_lyrics_song_id: Option<SongId>,

    /// 当前歌的振幅包络(db 有则随本段与 `current_song` 原子送达;计算未就绪时 `None`,
    /// 算完经一次 `current` 版本 bump 补发)。归属恒等于 `current_song`——server 端
    /// 组段时按当前曲过滤,client 直接采用无需再猜归属。
    pub current_envelope: Option<Envelope>,
}

#[cfg(test)]
mod tests {
    use super::{PlayMode, Repeat};

    #[test]
    fn dimensions_round_trip() {
        for m in [
            PlayMode::Sequential,
            PlayMode::Shuffle,
            PlayMode::RepeatAll,
            PlayMode::RepeatOne,
        ] {
            assert_eq!(PlayMode::from_dimensions(m.shuffle(), m.repeat()), m);
        }
    }

    #[test]
    fn shuffle_on_repeat_all_or_off_is_shuffle() {
        // 用户规则:随机本就一直循环,shuffle 开 + 整列循环(或不循环)都 == Shuffle。
        assert_eq!(
            PlayMode::from_dimensions(/*shuffle*/ true, Repeat::All),
            PlayMode::Shuffle
        );
        assert_eq!(
            PlayMode::from_dimensions(/*shuffle*/ true, Repeat::Off),
            PlayMode::Shuffle
        );
    }

    #[test]
    fn shuffle_on_repeat_one_is_repeat_one() {
        // 用户规则:shuffle 开 + 单曲循环 == RepeatOne。
        assert_eq!(
            PlayMode::from_dimensions(/*shuffle*/ true, Repeat::One),
            PlayMode::RepeatOne
        );
    }

    #[test]
    fn with_shuffle_toggles_dimension() {
        assert_eq!(PlayMode::Sequential.with_shuffle(true), PlayMode::Shuffle);
        assert_eq!(PlayMode::RepeatAll.with_shuffle(true), PlayMode::Shuffle);
        assert_eq!(PlayMode::Shuffle.with_shuffle(false), PlayMode::RepeatAll);
    }

    #[test]
    fn with_repeat_changes_loop_dimension() {
        assert_eq!(
            PlayMode::Sequential.with_repeat(Repeat::One),
            PlayMode::RepeatOne
        );
        assert_eq!(
            PlayMode::Shuffle.with_repeat(Repeat::One),
            PlayMode::RepeatOne
        );
        assert_eq!(
            PlayMode::Sequential.with_repeat(Repeat::All),
            PlayMode::RepeatAll
        );
    }

    /// `name` / `from_name` 对偶:四档 round-trip;且 `name` 与 Debug 名一致
    /// (历史会话库存的是 Debug 名,守住存量兼容)。
    #[test]
    fn play_mode_name_round_trips_and_matches_debug() {
        let mut m = PlayMode::Sequential;
        for _ in 0..4 {
            assert_eq!(PlayMode::from_name(m.name()), Some(m));
            assert_eq!(m.name(), format!("{m:?}"), "name 应与历史 Debug 落库值一致");
            m = m.cycle();
        }
    }

    /// `from_name`:未知名(脏数据)返回 `None`,不 panic 不猜。
    #[test]
    fn play_mode_from_name_rejects_garbage() {
        assert_eq!(PlayMode::from_name(""), None);
        assert_eq!(PlayMode::from_name("sequential"), None, "大小写敏感");
        assert_eq!(PlayMode::from_name("Shuffle "), None, "不容忍空白");
    }
}

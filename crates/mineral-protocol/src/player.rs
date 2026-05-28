//! 播放循环模式 + Server 端持有的播放状态快照。
//!
//! `PlayMode` 历史在 mineral-tui::playback;搬来这里因为 server 端也要决定
//! `next_song` / `prev_song`,且需要走 wire(`PlayerSnapshot` 的字段)。
//! glyph / label 这两个 UI 字面量跟着挪过来 —— 字符画放 protocol 不优雅,
//! 但避免 mineral-tui 跨 crate 加 inherent impl 的麻烦,**够用**。

use mineral_model::{PlayUrl, Song, SongId};
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

/// Server 端持有的「播放上下文」快照,client 重连后立刻拉一份镜像到 UI。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    /// 当前在播的歌(`None` 表示从未播过 / 已 stop)。
    pub current_song: Option<Song>,

    /// 当前歌的播放 URL 元信息(format / bitrate);transport 用。
    pub play_url: Option<PlayUrl>,

    /// 当前在播音频的来源(下载 / 缓存 / 远端);`None` = 未知。
    #[serde(default)]
    pub play_origin: Option<PlaybackOrigin>,

    /// 当前 queue 列表。
    pub queue: Vec<Song>,

    /// queue 中「当前歌」的位置(用于 prev/next 锚点)。
    pub queue_sel: usize,

    /// Shuffle 进入时保存的原序;非 Shuffle 状态恒 `None`。
    pub original_queue: Option<Vec<Song>>,

    /// 当前播放模式。
    pub play_mode: PlayMode,

    /// 当前歌的歌词原文(server 端缓存最新一首)。client 拿来解析成行。
    pub current_lyrics: Option<mineral_model::Lyrics>,

    /// 当前歌对应的 song_id,用于 client 端校验 lyrics 是否跟得上 current_song。
    pub current_lyrics_song_id: Option<SongId>,
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
}

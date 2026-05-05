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
}

/// Server 端持有的「播放上下文」快照,client 重连后立刻拉一份镜像到 UI。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    /// 当前在播的歌(`None` 表示从未播过 / 已 stop)。
    pub current_song: Option<Song>,

    /// 当前歌的播放 URL 元信息(format / bitrate);transport 用。
    pub play_url: Option<PlayUrl>,

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

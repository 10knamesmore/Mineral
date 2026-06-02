//! 服务端持有的「播放上下文」内部状态。[`crate::player::PlayerCore`] 用 `Mutex<State>` 包它,
//! 队列计算([`crate::queue`])与播放模式切换直接读写其字段。

use mineral_model::{PlayUrl, Song, SongId};
use mineral_protocol::{PlayMode, PlaybackOrigin, PlayerSnapshot};

use crate::download::Capturing;

/// 播放上下文。字段对 crate 内开放(队列计算 / 模式切换 / 播放编排直接读写)。
pub(crate) struct State {
    /// 当前在播的歌。
    pub(crate) current_song: Option<Song>,

    /// 当前歌的播放 URL(从 SongUrlReady 写入)。
    pub(crate) play_url: Option<PlayUrl>,

    /// 当前在播音频的来源(下载 / 缓存 / 远端);切歌时由 `play_song` 写入。
    pub(crate) play_origin: Option<PlaybackOrigin>,

    /// 当前队列(顺序模式 = 原序;shuffle 模式 = 洗过)。
    pub(crate) queue: Vec<Song>,

    /// 当前歌在 `queue` 中的下标。
    pub(crate) queue_sel: usize,

    /// shuffle 切换前的原始顺序,关 shuffle 时还原用;非 shuffle 模式下为 `None`。
    pub(crate) original_queue: Option<Vec<Song>>,

    /// 当前播放模式(顺序 / 单曲 / 列表循环 / shuffle)。
    pub(crate) play_mode: PlayMode,

    /// 当前歌的歌词(从 LyricsReady 写入)。
    pub(crate) current_lyrics: Option<mineral_model::Lyrics>,

    /// 当前 lyrics 配对的歌 id(对不上 current_song 时不返回)。
    pub(crate) current_lyrics_song_id: Option<SongId>,

    /// 正在预拉(已发起 SongUrl 任务、URL 尚未回来)的下一曲 id;URL 到达时据此认领。
    /// 切歌 / 采纳后复位,避免对同一 next 重复预拉。
    pub(crate) prefetch_fired_for: Option<SongId>,

    /// 当前正在 capture(边播边落盘)的曲;自然播完 → 入缓存,中途打断 → 删残件。
    /// 命中缓存直接本地播时为 `None`(无需 capture)。
    pub(crate) capturing: Option<Capturing>,

    /// 已预排进 rodio 队列、等当前曲播完无缝接续的下一曲及其记账(gapless)。
    pub(crate) queued: Option<crate::gapless::Queued>,
}

impl State {
    /// 空 State,所有字段取默认/空值。
    pub(crate) fn empty() -> Self {
        Self {
            current_song: None,
            play_url: None,
            play_origin: None,
            queue: Vec::new(),
            queue_sel: 0,
            original_queue: None,
            play_mode: PlayMode::default(),
            current_lyrics: None,
            current_lyrics_song_id: None,
            prefetch_fired_for: None,
            capturing: None,
            queued: None,
        }
    }

    /// 从内部 State 拷出一份 [`PlayerSnapshot`] 给 client(廉价 clone)。
    ///
    /// # Return:
    ///   当前播放上下文的快照。
    pub(crate) fn snapshot(&self) -> PlayerSnapshot {
        PlayerSnapshot {
            current_song: self.current_song.clone(),
            play_url: self.play_url.clone(),
            play_origin: self.play_origin,
            queue: self.queue.clone(),
            queue_sel: self.queue_sel,
            original_queue: self.original_queue.clone(),
            play_mode: self.play_mode,
            current_lyrics: self.current_lyrics.clone(),
            current_lyrics_song_id: self.current_lyrics_song_id.clone(),
        }
    }
}

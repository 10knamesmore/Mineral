//! 播放 view-model。状态由 [`mineral_audio::AudioHandle::snapshot`] 在每个 UI tick 灌入。

use mineral_audio::AudioSnapshot;
use mineral_model::{PlayUrl, Song};

/// 播放循环模式。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlayMode {
    /// 顺序播放(到底停止)。
    #[default]
    Sequential,
    /// 随机播放(stage 4 仅 label,无实际洗牌)。
    Shuffle,
    /// 整列循环。
    RepeatAll,
    /// 单曲循环。
    RepeatOne,
}

impl PlayMode {
    /// `m` 键循环到下一档。
    pub fn cycle(self) -> Self {
        match self {
            Self::Sequential => Self::Shuffle,
            Self::Shuffle => Self::RepeatAll,
            Self::RepeatAll => Self::RepeatOne,
            Self::RepeatOne => Self::Sequential,
        }
    }

    /// transport 模式按钮字形。
    pub fn glyph(self) -> &'static str {
        match self {
            Self::Sequential => "→",
            Self::Shuffle => "⇄",
            Self::RepeatAll => "↻∞",
            Self::RepeatOne => "↻¹",
        }
    }

    /// vol/mode/sort 行短标签。
    pub fn label(self) -> &'static str {
        match self {
            Self::Sequential => "seq",
            Self::Shuffle => "shuffle",
            Self::RepeatAll => "repeat-all",
            Self::RepeatOne => "repeat-one",
        }
    }
}

/// 播放视图模型;真值在 audio engine,这里只缓存 UI 当帧需要的字段。
#[derive(Clone, Debug)]
pub struct Playback {
    /// 当前曲目(没有就播不出来)。
    pub track: Option<Song>,
    /// 进度(ms)。
    pub position_ms: u64,
    /// 是否在播放。
    pub playing: bool,
    /// 音量 0..=100。
    pub volume_pct: u8,
    /// 播放模式。
    pub mode: PlayMode,

    /// 当前曲目解析后的 PlayUrl(format / bitrate / size)。
    /// 切歌时清成 `None`,PlayUrlReady 或 prefetch 命中后写入。transport 用它显 `fmt`。
    pub play_url: Option<PlayUrl>,
}

impl Playback {
    /// 默认播放状态(volume=66%)。
    pub fn new() -> Self {
        Self {
            track: None,
            position_ms: 0,
            playing: false,
            volume_pct: 66,
            mode: PlayMode::Sequential,
            play_url: None,
        }
    }

    /// 当前曲目时长(ms),没有 track 时返回 0。优先取 song 元数据,因为 decoder
    /// 探出 duration 比 song 元数据慢一帧、且部分容器探不出来。
    pub fn duration_ms(&self) -> u64 {
        self.track.as_ref().map_or(0, |t| t.duration_ms)
    }

    /// 进度比例,0..=10000 basis points(渲染层除以 10000.0 得 f64)。
    pub fn ratio_bps(&self) -> u16 {
        let dur = self.duration_ms();
        if dur == 0 {
            return 0;
        }
        let r = self.position_ms.saturating_mul(10_000) / dur;
        u16::try_from(r.min(10_000)).unwrap_or(10_000)
    }

    /// 把 audio engine 的 snapshot 灌进 view-model。
    pub fn apply_audio_snapshot(&mut self, snap: AudioSnapshot) {
        self.position_ms = snap.position_ms;
        self.playing = snap.playing;
        self.volume_pct = snap.volume_pct;
    }
}

impl Default for Playback {
    fn default() -> Self {
        Self::new()
    }
}

/// `mm:ss` 格式化(ms 输入)。
pub fn format_ms(ms: u64) -> String {
    let secs = ms / 1000;
    let m = secs / 60;
    let s = secs % 60;
    format!("{m}:{s:02}")
}

//! 播放 view-model。状态由 [`mineral_audio::AudioHandle::snapshot`] 在每个 UI tick 灌入。

use mineral_audio::{AudioBackend, AudioSnapshot};
use mineral_model::{PlayUrl, Song};
pub use mineral_protocol::PlayMode;

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

    /// 音频后端形态。`Null` 时顶栏显「无音频设备」徽标提示降级。
    pub audio_backend: AudioBackend,
}

impl Playback {
    /// 默认播放状态
    pub fn new() -> Self {
        Self {
            track: None,
            position_ms: 0,
            playing: false,
            volume_pct: 100,
            mode: PlayMode::Sequential,
            play_url: None,
            audio_backend: AudioBackend::Device,
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
        self.audio_backend = snap.backend;
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

#[cfg(test)]
mod tests {
    use mineral_model::{Song, SongId, SourceKind};

    use super::{Playback, format_ms};

    /// 造一个带 track(指定时长 + 进度)的 Playback。
    fn with_track(duration_ms: u64, position_ms: u64) -> Playback {
        let mut pb = Playback::new();
        pb.track = Some(Song {
            id: SongId::new(SourceKind::LOCAL, "t"),
            name: "t".to_owned(),
            artists: Vec::new(),
            album: None,
            duration_ms,
            cover_url: None,
            source_url: None,
        });
        pb.position_ms = position_ms;
        pb
    }

    /// `format_ms`:秒 / 分进位与零填充。
    #[test]
    fn format_ms_cases() {
        assert_eq!(format_ms(0), "0:00");
        assert_eq!(format_ms(75_000), "1:15");
        assert_eq!(format_ms(3_661_000), "61:01");
    }

    /// `ratio_bps`:0..=10000 basis points;无 track / dur 0 → 0;超出 clamp 到满。
    #[test]
    fn ratio_bps_cases() {
        assert_eq!(Playback::new().ratio_bps(), 0);
        assert_eq!(with_track(0, 100).ratio_bps(), 0);
        assert_eq!(with_track(1000, 0).ratio_bps(), 0);
        assert_eq!(with_track(1000, 500).ratio_bps(), 5000);
        assert_eq!(with_track(1000, 1000).ratio_bps(), 10_000);
        assert_eq!(with_track(1000, 5000).ratio_bps(), 10_000);
    }

    /// `duration_ms`:取 track 元数据,无 track → 0。
    #[test]
    fn duration_ms_from_track() {
        assert_eq!(Playback::new().duration_ms(), 0);
        assert_eq!(with_track(4242, 0).duration_ms(), 4242);
    }
}

//! 播放 view-model。状态由 [`mineral_audio::AudioHandle::snapshot`] 在每个 UI tick 灌入。

use mineral_audio::{AudioBackend, AudioSnapshot};
use mineral_model::{PlayUrl, Song};
pub use mineral_protocol::{PlayMode, PlaybackOrigin};

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

    /// 当前在播音频的来源(下载 / 缓存 / 远端);transport 用它显来源徽标。`None` = 未知。
    pub play_origin: Option<PlaybackOrigin>,

    /// 音频后端形态。`Null` 时顶栏显「无音频设备」徽标提示降级。
    pub audio_backend: AudioBackend,

    /// 当前曲目已缓冲比例(0..=10000 basis points)。本地 / 已缓存恒满;远端流式播放时
    /// 随下载推进。transport 进度条据此在播放头之后画一段更亮的「已缓冲」轨道。
    pub buffered_bps: u16,

    /// 当前曲目采样率(Hz),由 audio engine 实测灌入;0 = 未在播 / 未探出。transport 在 fmt 段显示。
    pub sample_rate_hz: u32,
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
            play_origin: None,
            audio_backend: AudioBackend::Device,
            buffered_bps: 0,
            sample_rate_hz: 0,
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
        self.buffered_bps = snap.buffered_bps;
        self.sample_rate_hz = snap.sample_rate_hz;
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

    /// gapless 无缝边界:current_song 翻成新曲 + position 归零时,进度条按**新曲**时长重缩放
    /// (分母取 `Song.duration_ms` 元数据),证明边界处 `position_ms` 的跳变是纯观感、不会出现
    /// 「新位置 ÷ 旧时长」的错配。守卫 gapless 依赖的这条耦合。
    #[test]
    fn ratio_rescales_cleanly_across_gapless_track_flip() {
        // 旧曲:200s,播到 180s → 9000 bps。
        let mut pb = with_track(200_000, 180_000);
        assert_eq!(pb.ratio_bps(), 9_000);
        // 无缝边界:翻成新曲(100s),position 重置到 0。
        pb.track = with_track(100_000, 0).track;
        pb.position_ms = 0;
        assert_eq!(pb.ratio_bps(), 0, "翻新曲后进度条应从头");
        // 新曲播到 50s:按新曲 100s 分母 → 5000(而非旧曲 200s 的 2500)。
        pb.position_ms = 50_000;
        assert_eq!(pb.ratio_bps(), 5_000, "进度应按新曲时长重缩放,非旧曲");
    }

    /// `apply_audio_snapshot`:position / playing / volume / backend / buffered_bps 全量灌入。
    #[test]
    fn apply_snapshot_propagates_buffered_bps() {
        let mut pb = Playback::new();
        let snap = mineral_audio::AudioSnapshot {
            playing: true,
            position_ms: 12_000,
            volume_pct: 55,
            buffered_bps: 7_500,
            ..mineral_audio::AudioSnapshot::default()
        };
        pb.apply_audio_snapshot(snap);
        assert!(pb.playing);
        assert_eq!(pb.position_ms, 12_000);
        assert_eq!(pb.volume_pct, 55);
        assert_eq!(pb.buffered_bps, 7_500);
    }
}

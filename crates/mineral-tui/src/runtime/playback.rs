//! 播放 view-model。状态由 [`mineral_audio::AudioHandle::snapshot`] 在每个 UI tick 灌入。

use mineral_audio::{AudioBackend, AudioSnapshot, Bps};
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

    /// 当前曲目已缓冲比例。本地 / 已缓存恒满;远端流式播放时随下载推进。
    /// transport 进度条据此在播放头之后画一段更亮的「已缓冲」轨道。
    pub buffered_bps: Bps,

    /// 当前曲目采样率(Hz),由 audio engine 实测灌入;0 = 未在播 / 未探出。transport 在 fmt 段显示。
    pub sample_rate_hz: u32,

    /// 当前曲目 decoder 实测时长(ms);0 = 未探出(刚起播 / 部分容器探不出)。
    /// 与 song 元数据时长([`Self::duration_ms`])是两个口径:顶换流场景二者之差
    /// 是判断歌词时间轴是否失真的依据。
    pub engine_duration_ms: u64,

    /// 下一曲 gapless 预排状态。transport 据此显预排标记。
    pub prefetch: Prefetch,
}

/// 下一曲 gapless 预排状态:audio snapshot next_* 字段在 view-model 侧的聚合。
#[derive(Clone, Copy, Debug, Default)]
pub struct Prefetch {
    /// 是否已预排进引擎队列(prefetch 已 append)。
    pub ready: bool,

    /// 缓冲比例(未预排恒零)。
    pub buffered_bps: Bps,

    /// 远端字节是否已下完(仅 capture 流会置 true;本地曲恒 false,
    /// 其「就绪」由缓冲已满表达,见 [`Prefetch::stage`])。
    pub download_complete: bool,
}

impl Prefetch {
    /// 归纳预排阶段(transport prefetch 标记的判定口径)。
    ///
    /// 「就绪」= 字节下完 **或** 缓冲已满:`download_complete` 仅 capture 流的 waiter
    /// 会置 true,本地曲 / 纯流式靠缓冲满格(本地 append 即满)兜住,
    /// 否则它们会永远卡在「拉取中」。
    ///
    /// # Return:
    ///   当前 [`PrefetchStage`]。
    pub fn stage(&self) -> PrefetchStage {
        if !self.ready {
            PrefetchStage::Idle
        } else if self.download_complete || self.buffered_bps.is_full() {
            PrefetchStage::Ready
        } else {
            PrefetchStage::Fetching
        }
    }
}

/// 下一曲 gapless 预排在 UI 视角的阶段,由 [`Prefetch::stage`] 归纳。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrefetchStage {
    /// 未预排(或预排尚未 append 进队列)。
    Idle,

    /// 已预排进队列,远端字节仍在拉取。
    Fetching,

    /// 已就绪:字节下完(capture)或缓冲已满(本地 / 纯流式),曲终可无缝接续。
    Ready,
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
            buffered_bps: Bps::ZERO,
            sample_rate_hz: 0,
            engine_duration_ms: 0,
            prefetch: Prefetch::default(),
        }
    }

    /// 当前曲目时长(ms),没有 track 时返回 0。优先取 song 元数据,因为 decoder
    /// 探出 duration 比 song 元数据慢一帧、且部分容器探不出来。
    ///
    /// 例外:**顶换流**([`mineral_model::PlayUrl::substituted`])的元数据时长描述的
    /// 是原源音频,不是实际在播的流——decoder 一探出实测就切实测(总时长 / 进度条 /
    /// 按比例 seek 随之对齐替身流);未探出前仍回落元数据。
    pub fn duration_ms(&self) -> u64 {
        if self.play_url.as_ref().is_some_and(|u| u.substituted) && self.engine_duration_ms > 0 {
            return self.engine_duration_ms;
        }
        self.track.as_ref().map_or(0, |t| t.duration_ms)
    }

    /// 播放进度比例(已播 ms / 总时长 ms;无 track / 时长未知恒零)。
    pub fn ratio_bps(&self) -> Bps {
        Bps::ratio(self.position_ms, self.duration_ms())
    }

    /// 把 audio engine 的 snapshot 灌进 view-model。
    pub fn apply_audio_snapshot(&mut self, snap: AudioSnapshot) {
        self.position_ms = snap.position_ms;
        self.playing = snap.playing;
        self.volume_pct = snap.volume_pct;
        self.audio_backend = snap.backend;
        self.buffered_bps = snap.buffered_bps;
        self.sample_rate_hz = snap.sample_rate_hz;
        self.engine_duration_ms = snap.duration_ms;
        self.prefetch = Prefetch {
            ready: snap.next_ready,
            buffered_bps: snap.next_buffered_bps,
            download_complete: snap.next_download_complete,
        };
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
    use mineral_audio::Bps;
    use mineral_model::{Song, SongId, SourceKind};

    use super::{Playback, format_ms};

    /// 造一个带 track(指定时长 + 进度)的 Playback。
    fn with_track(duration_ms: u64, position_ms: u64) -> Playback {
        let mut pb = Playback::new();
        pb.track = Some(
            Song::builder()
                .id(SongId::new(SourceKind::LOCAL, "t"))
                .name("t".to_owned())
                .duration_ms(duration_ms)
                .build(),
        );
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

    /// `ratio_bps`:无 track / dur 0 → 零;超出 clamp 到满。
    #[test]
    fn ratio_bps_cases() {
        assert_eq!(Playback::new().ratio_bps(), Bps::ZERO);
        assert_eq!(with_track(0, 100).ratio_bps(), Bps::ZERO);
        assert_eq!(with_track(1000, 0).ratio_bps(), Bps::ZERO);
        assert_eq!(with_track(1000, 500).ratio_bps(), Bps::new(5000));
        assert_eq!(with_track(1000, 1000).ratio_bps(), Bps::FULL);
        assert_eq!(with_track(1000, 5000).ratio_bps(), Bps::FULL);
    }

    /// `duration_ms`:取 track 元数据,无 track → 0。
    #[test]
    fn duration_ms_from_track() {
        assert_eq!(Playback::new().duration_ms(), 0);
        assert_eq!(with_track(4242, 0).duration_ms(), 4242);
    }

    /// `duration_ms` 顶换流口径:实测探出后切实测(元数据描述的是原源音频),
    /// 未探出前回落元数据;原源流即使实测在手也仍用元数据(探出慢一帧 + 部分容器探不出)。
    #[test]
    fn duration_ms_prefers_engine_for_substituted_stream() {
        let mut pb = with_track(296_533, 0);
        pb.play_url = Some(mineral_model::PlayUrl {
            song_id: SongId::new(SourceKind::NETEASE, "185868"),
            url: mineral_model::MediaUrl::Local("/sub.m4s".into()),
            bitrate_bps: 0,
            quality: mineral_model::BitRate::Standard,
            size: 0,
            format: mineral_model::AudioFormat::Aac,
            bit_depth: None,
            stream_headers: Vec::new(),
            layout: mineral_model::StreamLayout::Chunked,
            substituted: true,
        });
        assert_eq!(pb.duration_ms(), 296_533, "未探出前回落元数据");
        pb.engine_duration_ms = 298_000;
        assert_eq!(pb.duration_ms(), 298_000, "探出后以替身流实测为准");

        // 原源流:实测在手也不切口径。
        if let Some(pu) = pb.play_url.as_mut() {
            pu.substituted = false;
        }
        assert_eq!(pb.duration_ms(), 296_533, "原源流恒元数据");
    }

    /// gapless 无缝边界:current_song 翻成新曲 + position 归零时,进度条按**新曲**时长重缩放
    /// (分母取 `Song.duration_ms` 元数据),证明边界处 `position_ms` 的跳变是纯观感、不会出现
    /// 「新位置 ÷ 旧时长」的错配。守卫 gapless 依赖的这条耦合。
    #[test]
    fn ratio_rescales_cleanly_across_gapless_track_flip() {
        // 旧曲:200s,播到 180s → 9000 bps。
        let mut pb = with_track(200_000, 180_000);
        assert_eq!(pb.ratio_bps(), Bps::new(9_000));
        // 无缝边界:翻成新曲(100s),position 重置到 0。
        pb.track = with_track(100_000, 0).track;
        pb.position_ms = 0;
        assert_eq!(pb.ratio_bps(), Bps::ZERO, "翻新曲后进度条应从头");
        // 新曲播到 50s:按新曲 100s 分母 → 5000(而非旧曲 200s 的 2500)。
        pb.position_ms = 50_000;
        assert_eq!(
            pb.ratio_bps(),
            Bps::new(5_000),
            "进度应按新曲时长重缩放,非旧曲"
        );
    }

    /// `apply_audio_snapshot`:position / playing / volume / backend / buffered_bps 全量灌入。
    #[test]
    fn apply_snapshot_propagates_buffered_bps() {
        let mut pb = Playback::new();
        let snap = mineral_audio::AudioSnapshot {
            playing: true,
            position_ms: 12_000,
            volume_pct: 55,
            buffered_bps: Bps::new(7_500),
            ..mineral_audio::AudioSnapshot::default()
        };
        pb.apply_audio_snapshot(snap);
        assert!(pb.playing);
        assert_eq!(pb.position_ms, 12_000);
        assert_eq!(pb.volume_pct, 55);
        assert_eq!(pb.buffered_bps, Bps::new(7_500));
    }

    /// `apply_audio_snapshot`:next_* 预排字段聚合灌进 `prefetch`(transport 标记的数据源)。
    #[test]
    fn apply_snapshot_propagates_prefetch() {
        let mut pb = Playback::new();
        let snap = mineral_audio::AudioSnapshot {
            next_ready: true,
            next_buffered_bps: Bps::new(6_000),
            next_download_complete: true,
            ..mineral_audio::AudioSnapshot::default()
        };
        pb.apply_audio_snapshot(snap);
        assert!(pb.prefetch.ready);
        assert_eq!(pb.prefetch.buffered_bps, Bps::new(6_000));
        assert!(pb.prefetch.download_complete);
    }

    /// `Prefetch::stage` 三态归纳:未预排 → Idle;已预排未稳 → Fetching;
    /// 字节下完(capture)或缓冲已满(本地曲 append 即满 / 纯流式拉完)→ Ready。
    #[test]
    fn prefetch_stage_cases() {
        use super::{Prefetch, PrefetchStage};
        let mut pf = Prefetch::default();
        // 未预排:即便残留缓冲值也不算(engine 未占用槽恒 0,这里防御性钉死语义)。
        assert_eq!(pf.stage(), PrefetchStage::Idle);
        // 已预排、远端字节还在拉 → 拉取中。
        pf.ready = true;
        pf.buffered_bps = Bps::new(4_000);
        assert_eq!(pf.stage(), PrefetchStage::Fetching);
        // capture 字节下完 → 就绪。
        pf.download_complete = true;
        assert_eq!(pf.stage(), PrefetchStage::Ready);
        // 本地 / 纯流式:done_gen 不写,但缓冲已满 → 同样就绪。
        pf.download_complete = false;
        pf.buffered_bps = Bps::FULL;
        assert_eq!(pf.stage(), PrefetchStage::Ready);
    }
}

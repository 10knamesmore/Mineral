//! 播放 view-model。状态由 [`mineral_audio::AudioHandle::snapshot`] 在每个 UI tick 灌入。

use mineral_audio::{AudioBackend, AudioSnapshot, Bps};
use mineral_model::{Envelope, PlayUrl, Song, SongId};
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

    /// 当前曲目 decoder 实测时长(ms);`None` = 未探出(刚起播 / 分片容器流式打开探不出)。
    /// 与 song 元数据时长([`Self::duration_ms`])是两个口径:顶换流场景二者之差
    /// 是判断歌词时间轴是否失真的依据。
    pub engine_duration_ms: Option<u64>,

    /// 下一曲 gapless 预排状态。transport 据此显预排标记。
    pub prefetch: Prefetch,

    /// 当前曲的振幅包络(归属歌曲 id + 数据),随 `current` 重段([`mineral_protocol::CurrentSync`])
    /// 与 `track` **原子到达**。**读取走 [`Self::current_envelope`]**(只认归属当前 track 的),
    /// 换曲时旧包络被新段整体顶替、归属校验再兜一层,无需显式清空。
    pub envelope: Option<(SongId, Envelope)>,
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

/// 歌词时间轴的信任档。歌词永远来自歌曲原源,音频流却可能被拦截脚本顶换
/// ([`mineral_model::PlayUrl::substituted`])——顶换后时间轴是「借来的」,
/// 按实测时长差分档降级,不做伪精度对齐。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum SyncTrust {
    /// 原源流:时间轴与音频同源,完全可信。
    Native,

    /// 顶换流,实测时长与元数据相近(或尚未探出):保留同步,标识提示可能漂移。
    Borrowed,

    /// 顶换流且实测时长差超阈:时间轴确定失真,放弃逐行同步(静态呈现)。
    Broken,
}

/// 顶换流「时长差可容忍」阈值(ms):差在此内多半是同曲重传(常见于补救命中原版
/// 录音的搬运),同步仍基本可用;超过则视为不同版本,逐行高亮只会误导。
const SUBSTITUTED_DURATION_SLACK_MS: u64 = 2_000;

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
            engine_duration_ms: None,
            prefetch: Prefetch::default(),
            envelope: None,
        }
    }

    /// 当前曲的振幅包络;仅当已装载包络归属当前 track 时可见。
    ///
    /// 包络随 `current` 重段与 track 原子送达,故正常路径下二者恒一致;这层归属校验
    /// 只是对"包络尚未随新段更新的中间帧"兜底(返回 `None` 回落普通进度条)。
    ///
    /// # Return:
    ///   归属匹配返回 `Some(&Envelope)`,无包络 / 无 track / 归属不符返回 `None`。
    pub fn current_envelope(&self) -> Option<&Envelope> {
        let (owner, envelope) = self.envelope.as_ref()?;
        (self.track.as_ref()?.id == *owner).then_some(envelope)
    }

    /// 当前曲目时长(ms);`None` = 无 track / 两口径都未知。优先取 song 元数据,
    /// 因为 decoder 探出 duration 比 song 元数据慢一帧、且部分容器探不出来。
    ///
    /// 例外:**顶换流**([`mineral_model::PlayUrl::substituted`])的元数据时长描述的
    /// 是原源音频,不是实际在播的流——decoder 一探出实测就切实测(总时长 / 进度条 /
    /// 按比例 seek 随之对齐替身流);未探出前仍回落元数据。
    pub fn duration_ms(&self) -> Option<u64> {
        if self.play_url.as_ref().is_some_and(|u| u.substituted)
            && let Some(engine) = self.engine_duration_ms
        {
            return Some(engine);
        }
        self.track.as_ref().and_then(|t| t.duration_ms)
    }

    /// 播放进度比例(已播 ms / 总时长 ms;无 track / 时长未知恒零)。
    pub fn ratio_bps(&self) -> Bps {
        self.duration_ms()
            .map_or(Bps::ZERO, |total| Bps::ratio(self.position_ms, total))
    }

    /// 归纳当前播放的歌词时间轴信任档(判定口径见 [`SyncTrust`])。
    ///
    /// # Return:
    ///   时间轴信任档;歌词面板与窗口标题据此决定是否认「当前行」。
    pub(crate) fn sync_trust(&self) -> SyncTrust {
        if !self.play_url.as_ref().is_some_and(|u| u.substituted) {
            return SyncTrust::Native;
        }
        // 必须直读 song 元数据:`duration_ms()` 对顶换流已切实测口径,拿它比实测差恒 0。
        let meta = self.track.as_ref().and_then(|t| t.duration_ms);
        // 任一口径未知(刚起播 / 容器探不出 / 元数据缺)先按可用处理,探出后自然降档。
        if let (Some(meta), Some(engine)) = (meta, self.engine_duration_ms)
            && engine.abs_diff(meta) > SUBSTITUTED_DURATION_SLACK_MS
        {
            SyncTrust::Broken
        } else {
            SyncTrust::Borrowed
        }
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

#[cfg(test)]
mod tests {
    use mineral_audio::Bps;
    use mineral_model::{Song, SongId, SourceKind};

    use super::Playback;

    /// 造一个带 track(指定时长 + 进度)的 Playback。
    fn with_track(duration_ms: u64, position_ms: u64) -> Playback {
        let mut pb = Playback::new();
        pb.track = Some(
            Song::builder()
                .id(SongId::new(SourceKind::LOCAL, "t"))
                .name("t".to_owned())
                .duration_ms(Some(duration_ms))
                .build(),
        );
        pb.position_ms = position_ms;
        pb
    }

    /// 时间轴信任分档:原源流恒 Native;顶换流按「实测 vs 元数据」时长差分
    /// Borrowed / Broken,任一口径未探出先按 Borrowed(不预判失真)。
    #[test]
    fn sync_trust_tiers_by_duration_gap() {
        use super::SyncTrust;

        let play_url = |substituted: bool| mineral_model::PlayUrl {
            song_id: SongId::new(SourceKind::NETEASE, "1"),
            url: mineral_model::MediaUrl::Local("/x".into()),
            bitrate_bps: None,
            quality: mineral_model::BitRate::Standard,
            size: None,
            format: Some(mineral_model::AudioFormat::Mp3),
            bit_depth: None,
            stream_headers: Vec::new(),
            layout: mineral_model::StreamLayout::Contiguous,
            substituted,
        };
        let mut pb = with_track(/*duration_ms*/ 269_000, /*position_ms*/ 0);
        pb.play_url = Some(play_url(/*substituted*/ false));
        pb.engine_duration_ms = Some(300_000);
        assert_eq!(pb.sync_trust(), SyncTrust::Native, "原源流时长差不参与");

        pb.play_url = Some(play_url(/*substituted*/ true));
        pb.engine_duration_ms = None;
        assert_eq!(pb.sync_trust(), SyncTrust::Borrowed, "未探出先不判失真");
        pb.engine_duration_ms = Some(270_500);
        assert_eq!(pb.sync_trust(), SyncTrust::Borrowed, "差 1.5s 在容忍内");
        pb.engine_duration_ms = Some(280_000);
        assert_eq!(pb.sync_trust(), SyncTrust::Broken, "差 11s 判失真");
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

    /// `duration_ms`:取 track 元数据,无 track → `None`。
    #[test]
    fn duration_ms_from_track() {
        assert_eq!(Playback::new().duration_ms(), None);
        assert_eq!(with_track(4242, 0).duration_ms(), Some(4242));
    }

    /// `duration_ms` 顶换流口径:实测探出后切实测(元数据描述的是原源音频),
    /// 未探出前回落元数据;原源流即使实测在手也仍用元数据(探出慢一帧 + 部分容器探不出)。
    #[test]
    fn duration_ms_prefers_engine_for_substituted_stream() {
        let mut pb = with_track(296_533, 0);
        pb.play_url = Some(mineral_model::PlayUrl {
            song_id: SongId::new(SourceKind::NETEASE, "185868"),
            url: mineral_model::MediaUrl::Local("/sub.m4s".into()),
            bitrate_bps: None,
            quality: mineral_model::BitRate::Standard,
            size: None,
            format: Some(mineral_model::AudioFormat::Aac),
            bit_depth: None,
            stream_headers: Vec::new(),
            layout: mineral_model::StreamLayout::Chunked,
            substituted: true,
        });
        assert_eq!(pb.duration_ms(), Some(296_533), "未探出前回落元数据");
        pb.engine_duration_ms = Some(298_000);
        assert_eq!(pb.duration_ms(), Some(298_000), "探出后以替身流实测为准");

        // 原源流:实测在手也不切口径。
        if let Some(pu) = pb.play_url.as_mut() {
            pu.substituted = false;
        }
        assert_eq!(pb.duration_ms(), Some(296_533), "原源流恒元数据");
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

    /// 包络只在归属当前曲时可见:匹配可见、异曲包络(迟到事件)不可见、
    /// 换曲后旧包络自然失效——无需任何显式清空点。
    #[test]
    fn envelope_visible_only_for_matching_track() -> color_eyre::Result<()> {
        use mineral_model::Envelope;

        let mut pb = with_track(1000, 0);
        let current_id = pb
            .track
            .as_ref()
            .map(|t| t.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("with_track 必有 track"))?;
        assert!(pb.current_envelope().is_none(), "未装载时不可见");

        pb.envelope = Some((
            current_id,
            Envelope {
                points: vec![9],
                version: 1,
            },
        ));
        assert!(pb.current_envelope().is_some(), "归属当前曲应可见");

        // 换曲(不同 id):旧包络自然失效。
        pb.track = Some(
            Song::builder()
                .id(SongId::new(SourceKind::LOCAL, "another"))
                .name("another".to_owned())
                .duration_ms(Some(1000))
                .build(),
        );
        assert!(pb.current_envelope().is_none(), "换曲后旧包络不可见");
        Ok(())
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

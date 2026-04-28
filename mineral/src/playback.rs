//! 播放状态机(mock,无真实音频) — 用 tick 推进 [`Playback::position_ms`]。
//!
//! 全部用整数算术(`u64` ms / `u8` 0..=100 vol),避开 `as` / 浮点精度问题,
//! 渲染层把整数换成 `f64` ratio 喂 ratatui Gauge / 自绘进度条。

use std::time::Duration;

use mineral_model::Song;

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

/// 播放状态
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
    /// 输出设备名。
    pub device: String,
    /// 当前流的格式描述(mock)。
    pub format: String,
}

impl Playback {
    /// 默认 mock 播放状态(volume=66%, device="HD-650")。
    pub fn new() -> Self {
        Self {
            track: None,
            position_ms: 0,
            playing: false,
            volume_pct: 66,
            mode: PlayMode::Sequential,
            device: "HD-650".to_owned(),
            format: "24/96 flac".to_owned(),
        }
    }

    /// 当前曲目时长(ms),没有 track 时返回 0。
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

    /// 把现实流逝的时间推进到播放进度上。
    pub fn tick(&mut self, dt: Duration) {
        if !self.playing {
            return;
        }
        let dt_ms = u64::try_from(dt.as_millis()).unwrap_or(0);
        let new_pos = self.position_ms.saturating_add(dt_ms);
        let dur = self.duration_ms();
        if dur > 0 && new_pos >= dur {
            if self.mode == PlayMode::RepeatOne {
                self.position_ms = 0;
            } else {
                self.position_ms = dur;
                self.playing = false;
            }
        } else {
            self.position_ms = new_pos;
        }
    }

    /// 切换 play / pause(没有 track 时不动)。
    pub fn play_pause(&mut self) {
        if self.track.is_some() {
            self.playing = !self.playing;
        }
    }

    /// 调整音量(`delta` 单位 = 百分点,可正可负)。
    pub fn nudge_volume(&mut self, delta: i8) {
        let v = i32::from(self.volume_pct).saturating_add(i32::from(delta));
        self.volume_pct = u8::try_from(v.clamp(0, 100)).unwrap_or(self.volume_pct);
    }

    /// 跳转(`delta_s` 单位 = 秒,可正可负)。
    pub fn seek(&mut self, delta_s: i32) {
        let dur = self.duration_ms();
        let delta_ms = i64::from(delta_s).saturating_mul(1000);
        let cur = i64::try_from(self.position_ms).unwrap_or(0);
        let max = i64::try_from(dur).unwrap_or(0);
        let new_ms = cur.saturating_add(delta_ms).clamp(0, max);
        self.position_ms = u64::try_from(new_ms).unwrap_or(0);
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

//! 频谱面板的每帧供数:PCM 喂入 + 按渲染风格分路消费,以及封面配色协调。

use crate::app::App;

impl App {
    /// 协调当前播放封面的频谱配色:新封面取色就绪则从**当前可见配色**缓动过去,否则保持现状。
    ///
    /// 身份判定(`cover_url` 变化、色带是否就绪)全在此(app 层);频谱只收
    /// `begin_cover_transition` / `clear_cover` 两个命令,不持有歌曲 / URL 身份。
    ///
    /// - 当前封面与 `spectrum_cover` 一致 → 不动。
    /// - 当前封面变了 + 色带已就绪 → `begin_cover_transition`(从上一张封面 / hue 起步),记下 key。
    /// - 当前封面变了 + 图已到但**取色失败**(在 `covers.cache` 却不在 `covers.palettes`)→ 回退 hue,标记已处理。
    /// - 当前封面变了 + 图还在抓 → **保持当前可见态**(上一张封面继续显示),下个 tick 再看。
    ///   这是"红专辑换蓝专辑 → 红→蓝"的关键:抓图途中不回退 hue,等蓝就绪直接红→蓝。
    /// - 无当前歌 / 无封面 → 回退 hue。
    pub(super) fn sync_spectrum_palette(&mut self) {
        let cur = self
            .state
            .player
            .current
            .as_ref()
            .and_then(|s| s.cover_url.clone());
        let Some(url) = cur else {
            if self.state.covers.spectrum_cover.is_some() {
                self.state.spectrum.clear_cover();
                self.state.covers.spectrum_cover = None;
                self.state.covers.current_palette = None;
            }
            return;
        };
        if self.state.covers.spectrum_cover.as_ref() == Some(&url) {
            return;
        }
        if let Some(palette) = self.state.covers.palettes.get(&url).cloned() {
            self.state
                .spectrum
                .begin_cover_transition(palette.clone(), &self.theme);
            self.state.covers.spectrum_cover = Some(url);
            self.state.covers.current_palette = Some(palette);
        } else if self.state.covers.cache.contains_key(&url) {
            // 图已回但无色板 = 取色失败:回退 hue,标记已处理(不再每帧重试)。
            self.state.spectrum.clear_cover();
            self.state.covers.spectrum_cover = Some(url);
            self.state.covers.current_palette = None;
        }
        // else:封面还在抓,保持当前可见态(上一张封面 / hue)不动,等就绪后再红→蓝。
    }

    /// 把 client.pull_pcm 拿到的样本喂给 fft computer。in-proc 和 connect 走同一路径。
    /// 消费口按渲染风格分路:scope 直接吃时域样本(state 侧聚合成滚动包络),
    /// 其余走 FFT 条高。样本无条件进 FFT 环形窗,风格切回条形家族时窗还是热的。
    pub(super) fn update_spectrum(&mut self) {
        // 每 tick 最多拉一个 FFT 窗的样本:正常一帧只来几百样本,卡顿后一帧即可补满整窗。
        let pop_chunk = self.state.fft.window_size();
        let (samples, sample_rate) = self.client.pull_pcm(pop_chunk);
        if !samples.is_empty() {
            self.state.fft.push(&samples);
        }
        let playing = self.state.playback.playing;
        let volume_pct = self.state.playback.volume_pct;
        if *self.state.cfg.tui().spectrum().style() == mineral_config::SpectrumStyle::Scope {
            self.state
                .spectrum
                .tick_scope(volume_pct, &samples, sample_rate);
        } else {
            let target_bars = self.state.spectrum.target_bars.get();
            let bars = self.state.fft.compute(sample_rate, target_bars);
            self.state
                .spectrum
                .tick(playing, volume_pct, bars.as_deref());
        }
    }
}

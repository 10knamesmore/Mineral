//! 频谱面板的每帧供数:PCM 喂入 + 按渲染风格分路消费。
//! (封面配色协调在 `cover_colors`——频谱色场只是封面色的消费方之一。)

use crate::app::App;

impl App {
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

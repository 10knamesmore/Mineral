//! 频谱面板的每帧供数:PCM 镜像窗口消费 + 按渲染风格分路。
//! (封面配色协调在 `crate::image::accent`——频谱色场只是封面色的消费方之一。)

use crate::app::App;

impl App {
    /// 把 client 的 PCM 近期窗口喂给 fft computer。所有启动模式走同一路径。
    /// 消费口按渲染风格分路:scope 直接吃时域样本(state 侧聚合成滚动包络),
    /// 其余走 FFT 条高。样本无条件进 FFT 环形窗,风格切回条形家族时窗还是热的。
    pub(super) fn update_spectrum(&mut self) {
        let samples = self.client.drain_pcm();
        // 换代 / 缺口:重置需要连续窗口的 FFT 与 scope 状态;动画衰减照常推进。
        if self.client.take_pcm_discontinuity() {
            self.state.fft.reset();
            self.state.spectrum.reset_stream();
        }
        let sample_rate = self.state.playback.sample_rate_hz;
        if !samples.is_empty() {
            self.state.fft.push(&samples);
        }
        // 同一批样本顺路喂氛围背景的响度包络(空样本 = 静音,包络自然回落)。
        self.ambient_pulse.feed(
            &samples,
            sample_rate,
            self.state.cfg.tui().ambient().pulse(),
        );
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

//! Server 端 PCM 中继:把 [`SpectrumTap`] 收纳到 server 内部,client 通过 `pull` 拉。
//!
//! 解决「`SpectrumTap` 跨不了进程」的限制 —— 让 in-proc 和 connect 走同一接口。
//!
//! ## 实现:on-demand pull(无中间 buffer)
//!
//! audio engine 内部的 SPSC ring 已有 4096 sample (~85ms @ 48kHz) 缓冲,远大于
//! client 30fps tick (33ms) 的间隔。中间再加一层 worker buffer 是冗余 —— 直接
//! `pull` 时 lock + `pop_into` 即可。
//!
//! 历史曾用 16ms tick worker drain 到 VecDeque,但那一层引入最坏 16ms 额外延迟,
//! spectrum 看着会「慢半拍」。on-demand pull 让最坏延迟 ≈ 一个 client tick gap (~5ms)。

use std::sync::Arc;

use mineral_audio::SpectrumTap;
use parking_lot::Mutex;

/// PCM 中继。`Clone` 廉价(`Arc<Mutex>` 共享同一个 SPSC consumer)。
#[derive(Clone)]
pub(crate) struct PcmPuller {
    /// SPSC consumer 单一持有者,Mutex 是为了让 `Clone` 共享(实际只会被
    /// `pull` 调用串行 lock,无并发争用)。
    tap: Arc<Mutex<SpectrumTap>>,
}

impl PcmPuller {
    /// 接管 `SpectrumTap` 的 ownership。无后台 task。
    pub fn spawn(tap: SpectrumTap) -> Self {
        Self {
            tap: Arc::new(Mutex::new(tap)),
        }
    }

    /// 拉最多 N 个 sample(可能短于 N)+ 当前 sample_rate(0 = 没在播)。
    pub fn pull(&self, n: usize) -> (Vec<f32>, u32) {
        let mut tap = self.tap.lock();
        let mut buf = vec![0_f32; n];
        let got = tap.pop_into(&mut buf);
        buf.truncate(got);
        let sr = tap.sample_rate();
        (buf, sr)
    }
}

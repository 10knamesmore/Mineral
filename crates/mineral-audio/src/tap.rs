//! 把 rodio decoder 包一层,样本透传同时把 mono 化的 f32 副本写进 SPSC ringbuf,
//! 供 UI 端的 spectrum FFT 消费。
//!
//! `Iterator::next` 在 cpal mixer 回调链路上,**绝对不能阻塞**:满了就丢
//! (用户视角:旧样本对可视化无价值)。`sample_rate` 在构造时写进
//! `Arc<AtomicU32>`,UI 端按需读取,跟随每首曲目变化。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use ringbuf::HeapProd;
use ringbuf::traits::Producer;
use rodio::source::SeekError;
use rodio::{ChannelCount, SampleRate, Source};

/// 共享 producer 别名:一个 engine 生命期内多次切歌共用同一个 ringbuf 写端。
///
/// SPSC 的 producer 不可 clone,但每首歌要新建一个 [`TapSource`] 包新 decoder ——
/// 用 `Arc<Mutex<HeapProd>>` 共享所有权。同一时刻只有 audio 线程持有它,
/// `parking_lot` 的非竞争 lock ~5ns,远小于一个 sample 的播放间隔。
pub(crate) type SharedProd = Arc<Mutex<HeapProd<f32>>>;

/// 包装 `Source<Item = f32>`,样本流透传 + mono 副本旁路。
pub(crate) struct TapSource<S> {
    /// 内层音频源(decoder 或 stream)。
    inner: S,

    /// 共享 ringbuf 写端。满了 try_push 直接 drop。
    producer: SharedProd,

    /// 通道数。`Source::channels` 返回 `NonZero<u16>`,这里缓存原生 u16 方便累加。
    channels: u16,

    /// 当前帧累加的样本和(L+R+...)。
    accum: f32,

    /// 当前帧已累加的样本数,达到 `channels` 就 push 平均值。
    samples_in_frame: u16,
}

impl<S> TapSource<S>
where
    S: Source<Item = f32>,
{
    /// 包装 `inner`,把它的 sample_rate 写进 `sr_atomic` 供 UI 端读取。
    pub(crate) fn new(inner: S, producer: SharedProd, sr_atomic: &Arc<AtomicU32>) -> Self {
        sr_atomic.store(u32::from(inner.sample_rate()), Ordering::Relaxed);
        let channels = u16::from(inner.channels());
        Self {
            inner,
            producer,
            channels,
            accum: 0.0,
            samples_in_frame: 0,
        }
    }
}

impl<S> Iterator for TapSource<S>
where
    S: Source<Item = f32>,
{
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let s = self.inner.next()?;
        self.accum += s;
        self.samples_in_frame = self.samples_in_frame.saturating_add(1);
        let n = self.channels.max(1);
        if self.samples_in_frame >= n {
            let avg = self.accum / f32::from(n);
            // 满了就丢,绝不阻塞 mixer 回调线程。lock 非竞争 ≈ 5ns。
            let _ = self.producer.lock().try_push(avg);
            self.accum = 0.0;
            self.samples_in_frame = 0;
        }
        Some(s)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S> Source for TapSource<S>
where
    S: Source<Item = f32>,
{
    fn current_span_len(&self) -> Option<usize> {
        self.inner.current_span_len()
    }

    fn channels(&self) -> ChannelCount {
        self.inner.channels()
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    /// 透传给 inner decoder。**关键**:不实现这条会回落到 `Source::try_seek` 的默认
    /// 实现,直接返回 `SeekError::NotSupported`,导致 ←/→ 进度全失效。
    fn try_seek(&mut self, pos: Duration) -> Result<(), SeekError> {
        self.inner.try_seek(pos)
    }
}

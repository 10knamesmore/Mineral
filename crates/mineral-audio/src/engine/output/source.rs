//! 在旧设备流销毁后归还播放队列，让新流接管同一 decoder 和预排曲目。

use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use rodio::Source;
use rodio::queue::SourcesQueueOutput;
use rodio::source::{SeekError, UniformSourceIterator};

/// 流之间移交的唯一队列所有权；正在工作的回调独占队列，逐样本读取不加锁。
pub(super) type QueueHandoff = Arc<Mutex<Option<SourcesQueueOutput>>>;

/// 回调采用的声道和采样率转换器；转换由 rodio 负责。
pub(super) type OutputSamples = UniformSourceIterator<ReturningQueue>;

/// 尝试接管已归还的队列；旧流仍持有队列时，新流先输出静音。
pub(super) fn take_samples(
    handoff: &QueueHandoff,
    channels: rodio::ChannelCount,
    sample_rate: rodio::SampleRate,
) -> Option<OutputSamples> {
    let source = handoff.lock().take()?;
    Some(UniformSourceIterator::new(
        ReturningQueue {
            source: Some(source),
            handoff: Arc::clone(handoff),
        },
        channels,
        sample_rate,
    ))
}

/// 由一个 CPAL 回调拥有，回调销毁时将仍未播完的队列归还给输出管理器。
pub(super) struct ReturningQueue {
    /// 只在 `Drop` 时取走，因此活跃读取期间始终为 `Some`。
    source: Option<SourcesQueueOutput>,

    /// 下一条输出流取回队列的位置。
    handoff: QueueHandoff,
}

impl ReturningQueue {
    /// 借用活跃队列；队列只在本对象销毁时移出。
    #[allow(clippy::expect_used)] // The queue is taken only by Drop, after all Source calls end.
    fn source(&self) -> &SourcesQueueOutput {
        self.source
            .as_ref()
            .expect("output queue is present until drop")
    }
}

impl Iterator for ReturningQueue {
    type Item = rodio::Sample;

    fn next(&mut self) -> Option<Self::Item> {
        self.source.as_mut()?.next()
    }
}

impl Source for ReturningQueue {
    fn current_span_len(&self) -> Option<usize> {
        self.source().current_span_len()
    }

    fn channels(&self) -> rodio::ChannelCount {
        self.source().channels()
    }

    fn sample_rate(&self) -> rodio::SampleRate {
        self.source().sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.source().total_duration()
    }

    #[allow(clippy::expect_used)] // The queue is taken only by Drop, after all Source calls end.
    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.source
            .as_mut()
            .expect("output queue is present until drop")
            .try_seek(position)
    }
}

impl Drop for ReturningQueue {
    fn drop(&mut self) {
        *self.handoff.lock() = self.source.take();
    }
}

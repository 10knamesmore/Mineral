//! Server 端 PCM 中继:一个后台节奏从 audio tap 取新样本,按订阅者有界分发。
//!
//! 性质:
//! - **主动推送**:音频线程只做 mono 旁路写 ring;中继 task 每 [`PCM_TICK_MS`] 取一次
//!   并主动下发,不依赖 client 拉取。
//! - **有界**:每个订阅者一条小容量通道;慢了就丢新块并给下一块打缺口标记,不追赶旧样本。
//! - **可辨认断续**:播放代次(切歌 / seek / 格式变化)与上游缺口(ring 满丢样本)都会
//!   体现为 `generation` 变化或 `gap = true`。
//! - **无消费者停发**:没有订阅者时不构造任何块;重新订阅从当前近期窗口开始。

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use mineral_audio::{AudioHandle, SpectrumTap};
use mineral_protocol::PcmChunk;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use tokio::sync::mpsc;

/// 中继取样节奏。
pub(crate) const PCM_TICK_MS: u64 = 16;

/// 近期窗口样本上限(重订阅从这里开始,不回放更久之前的样本)。
const MAX_HISTORY: usize = 16 * 1024;

/// 单块最多样本数(约 46ms @44.1kHz;与 TUI 帧率解耦)。
const MAX_CHUNK: usize = 2048;

/// 每个订阅者可积压的块数。
pub(crate) const SUBSCRIBER_QUEUE_CHUNKS: usize = 8;

/// 判定 seek 的位置跳变阈值(ms)。
const POSITION_JUMP_MS: u64 = 1000;

/// 订阅令牌(会话断开 / 退订时移除游标)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PcmSubscription(u64);

impl PcmSubscription {
    /// 裸值(日志用)。
    #[cfg(test)]
    pub(crate) fn value(self) -> u64 {
        self.0
    }
}

/// 单个订阅者的有界游标。
struct Subscriber {
    /// 下一个要发送的绝对样本位置。
    next_position: u64,

    /// 下一块是否要带缺口标记(新订阅 / 丢块 / 上游缺口)。
    gap_pending: bool,

    /// 有界发送端(容量 [`SUBSCRIBER_QUEUE_CHUNKS`])。
    tx: mpsc::Sender<PcmChunk>,
}

/// 中继共享态(中继 task 与订阅 / 退订共用)。
struct RelayState {
    /// 已 drain 的绝对样本数(流位置尾)。
    consumed: u64,

    /// 播放代次(切歌 / seek / 格式变化 +1)。
    generation: u64,

    /// 上次观察到的音频轨道令牌。
    track_token: u64,

    /// 上次观察到的采样率(`None` = 尚未就绪)。
    sample_rate: Option<u32>,

    /// 按已 drain 样本推算的播放位置(ms)。
    expected_position_ms: u64,

    /// 近期窗口(尾 = `consumed`)。
    history: VecDeque<f32>,

    /// 各订阅者游标。
    subscribers: FxHashMap<u64, Subscriber>,

    /// 下一个订阅令牌。
    next_subscription: u64,
}

impl RelayState {
    /// 换代:清空窗口并让所有订阅者下一块带缺口。
    fn bump_generation(&mut self) {
        self.generation = self.generation.saturating_add(1);
        self.history.clear();
        for subscriber in self.subscribers.values_mut() {
            subscriber.gap_pending = true;
            subscriber.next_position = self.consumed;
        }
    }

    /// 注入一批样本,维护窗口与流位置。
    fn ingest(&mut self, samples: &[f32]) {
        self.history.extend(samples.iter().copied());
        self.consumed = self
            .consumed
            .saturating_add(u64::try_from(samples.len()).unwrap_or(u64::MAX));
        let overflow = self.history.len().saturating_sub(MAX_HISTORY);
        if overflow > 0 {
            self.history.drain(..overflow);
        }
    }
}

/// 一次取样的输入(生产路径来自 audio 引擎;测试可直接脚本化)。
struct RelayInput {
    /// 当前轨道令牌(切歌辨认)。
    track_token: u64,

    /// 引擎报告的播放位置(ms)。
    position_ms: u64,

    /// 是否在播。
    playing: bool,

    /// 当前采样率(0 = 尚未就绪)。
    sample_rate: u32,

    /// 累计写入 tap 的样本数(含因环满丢弃的)。
    pushed_total: u64,

    /// 本轮从 tap 取到的样本。
    samples: Vec<f32>,
}

/// PCM 中继句柄。`Clone` 廉价。
#[derive(Clone)]
pub(crate) struct PcmRelay {
    /// 共享态。
    inner: Arc<Mutex<RelayState>>,
}

impl PcmRelay {
    /// 接管 tap,并起后台分发 task。
    ///
    /// # Params:
    ///   - `tap`: audio 引擎的 mono 样本旁路
    ///   - `audio`: 音频句柄(读轨道令牌 / 位置以辨认换代)
    pub(crate) fn spawn(tap: SpectrumTap, audio: AudioHandle) -> Self {
        let relay = Self {
            inner: Arc::new(Mutex::new(RelayState {
                consumed: 0,
                generation: 1,
                track_token: 0,
                sample_rate: None,
                expected_position_ms: 0,
                history: VecDeque::new(),
                subscribers: FxHashMap::default(),
                next_subscription: 0,
            })),
        };
        let pump = relay.clone();
        tokio::spawn(async move { pump.run(audio, tap).await });
        relay
    }

    /// 造一个不接 audio 的中继(测试直接注入脚本化取样,不起后台 task)。
    #[cfg(test)]
    pub(crate) fn detached_for_test() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RelayState {
                consumed: 0,
                generation: 1,
                track_token: 0,
                sample_rate: None,
                expected_position_ms: 0,
                history: VecDeque::new(),
                subscribers: FxHashMap::default(),
                next_subscription: 0,
            })),
        }
    }

    /// 建立一个订阅:从当前近期窗口开始,首块带缺口标记。
    ///
    /// # Return:
    ///   (令牌, 有界接收端)。
    pub(crate) fn subscribe(&self) -> (PcmSubscription, mpsc::Receiver<PcmChunk>) {
        let mut state = self.inner.lock();
        let token = state.next_subscription;
        state.next_subscription = state.next_subscription.wrapping_add(1);
        let (tx, rx) = mpsc::channel(SUBSCRIBER_QUEUE_CHUNKS);
        let window_start = state
            .consumed
            .saturating_sub(u64::try_from(state.history.len()).unwrap_or(u64::MAX));
        state.subscribers.insert(
            token,
            Subscriber {
                next_position: window_start,
                gap_pending: true,
                tx,
            },
        );
        (PcmSubscription(token), rx)
    }

    /// 移除订阅游标(会话断开 / 退订)。
    ///
    /// # Params:
    ///   - `token`: 订阅令牌
    pub(crate) fn unsubscribe(&self, token: PcmSubscription) {
        self.inner.lock().subscribers.remove(&token.0);
    }

    /// 当前订阅者数(测试 / 指标)。
    #[cfg(test)]
    pub(crate) fn subscriber_count(&self) -> usize {
        self.inner.lock().subscribers.len()
    }

    /// 当前播放代次(测试 / 日志)。
    #[cfg(test)]
    pub(crate) fn generation(&self) -> u64 {
        self.inner.lock().generation
    }

    /// 后台节奏:取样 → 维护窗口 → 按订阅者切块推送。
    async fn run(self, audio: AudioHandle, mut tap: SpectrumTap) {
        let mut tick = tokio::time::interval(Duration::from_millis(PCM_TICK_MS));
        let mut scratch = [0_f32; 4096];
        loop {
            tick.tick().await;
            let snapshot = audio.snapshot();
            let mut samples = Vec::new();
            loop {
                let got = tap.pop_into(&mut scratch);
                if got == 0 {
                    break;
                }
                if let Some(batch) = scratch.get(..got) {
                    samples.extend_from_slice(batch);
                }
                if got < scratch.len() {
                    break;
                }
            }
            self.pump(&RelayInput {
                track_token: snapshot.current_track_token,
                position_ms: snapshot.position_ms,
                playing: snapshot.playing,
                sample_rate: tap.sample_rate(),
                pushed_total: tap.pushed_total(),
                samples,
            });
        }
    }

    /// 一次取样与分发(锁内完成,样本量小)。
    ///
    /// # Params:
    ///   - `input`: 本次取样的轨道事实与样本
    fn pump(&self, input: &RelayInput) {
        let mut state = self.inner.lock();
        // 切歌:轨道令牌变化即换代。
        if input.track_token != state.track_token {
            state.track_token = input.track_token;
            state.bump_generation();
            state.expected_position_ms = input.position_ms;
        }
        // 采样率变化:格式切换即换代(0 = 尚未就绪,不当作变化)。
        if input.sample_rate != 0 {
            match state.sample_rate {
                Some(previous) if previous != input.sample_rate => {
                    state.sample_rate = Some(input.sample_rate);
                    state.bump_generation();
                }
                None => state.sample_rate = Some(input.sample_rate),
                Some(_) => {}
            }
        }

        // 注入样本;若生产者尝试写入超过已消费量,说明上游丢过样本。
        let mut drained = 0_u64;
        for batch in input.samples.chunks(4096) {
            state.ingest(batch);
            drained = drained.saturating_add(u64::try_from(batch.len()).unwrap_or(u64::MAX));
        }
        let dropped_upstream = input.pushed_total.saturating_sub(state.consumed);
        if dropped_upstream > 0 {
            // 上游丢过样本:跳过它们并让下一块带缺口。
            state.consumed = input.pushed_total;
            for subscriber in state.subscribers.values_mut() {
                subscriber.gap_pending = true;
            }
        }
        if drained > 0 {
            let rate = state.sample_rate.unwrap_or(0);
            if rate > 0 {
                state.expected_position_ms = state
                    .expected_position_ms
                    .saturating_add(drained.saturating_mul(1000) / u64::from(rate));
            }
        }
        // seek:实际位置与推算位置偏离超过阈值即换代。
        if input.playing
            && rate_known(&state)
            && input.position_ms.abs_diff(state.expected_position_ms) > POSITION_JUMP_MS
        {
            state.expected_position_ms = input.position_ms;
            state.bump_generation();
        }

        if state.subscribers.is_empty() {
            return;
        }
        let sample_rate = state.sample_rate;
        let generation = state.generation;
        let consumed = state.consumed;
        // 取出窗口再迭代订阅者:两者同属 state,借用无法拆分。
        let history = std::mem::take(&mut state.history);
        let window_start =
            consumed.saturating_sub(u64::try_from(history.len()).unwrap_or(u64::MAX));
        let mut to_drop = Vec::new();
        for (token, subscriber) in state.subscribers.iter_mut() {
            if subscriber.next_position < window_start {
                subscriber.next_position = window_start;
                subscriber.gap_pending = true;
            }
            let available = consumed.saturating_sub(subscriber.next_position);
            if available == 0 {
                continue;
            }
            let take = usize::try_from(available)
                .unwrap_or(usize::MAX)
                .min(MAX_CHUNK);
            let offset =
                usize::try_from(subscriber.next_position.saturating_sub(window_start)).unwrap_or(0);
            let samples = history
                .iter()
                .skip(offset)
                .take(take)
                .copied()
                .collect::<Vec<f32>>();
            let chunk = PcmChunk {
                generation,
                position: subscriber.next_position,
                gap: subscriber.gap_pending,
                sample_rate,
                samples,
            };
            let len = u64::try_from(chunk.samples.len()).unwrap_or(u64::MAX);
            match subscriber.tx.try_send(chunk) {
                Ok(()) => {
                    subscriber.next_position = subscriber.next_position.saturating_add(len);
                    subscriber.gap_pending = false;
                }
                Err(mpsc::error::TrySendError::Full(_)) => {
                    // 订阅者慢:丢这一块,下一块带缺口,不追赶旧样本。
                    subscriber.gap_pending = true;
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    to_drop.push(*token);
                }
            }
        }
        for token in to_drop {
            state.subscribers.remove(&token);
        }
        state.history = history;
    }
}

/// 采样率是否已就绪。
fn rate_known(state: &RelayState) -> bool {
    state.sample_rate.is_some_and(|rate| rate > 0)
}

#[cfg(test)]
mod tests {
    use tokio::sync::mpsc;

    use super::{MAX_CHUNK, MAX_HISTORY, PcmChunk, PcmRelay, RelayInput, SUBSCRIBER_QUEUE_CHUNKS};

    /// 造一个不接 audio 的中继:直接 `pump` 脚本化取样。
    fn relay() -> PcmRelay {
        PcmRelay::detached_for_test()
    }

    /// 造一轮脚本化取样(默认 44.1kHz / 在播 / 轨道 1)。
    fn input(samples: usize) -> RelayInput {
        RelayInput {
            track_token: 1,
            position_ms: 0,
            playing: true,
            sample_rate: 44_100,
            pushed_total: 0,
            samples: vec![0.25; samples],
        }
    }

    /// 取出一批块,直到队列空。
    fn drain(rx: &mut mpsc::Receiver<PcmChunk>) -> Vec<PcmChunk> {
        let mut out = Vec::new();
        while let Ok(chunk) = rx.try_recv() {
            out.push(chunk);
        }
        out
    }

    /// 无消费者时订阅数为 0;订阅后为 1;退订回落;令牌与代次可从外部读取。
    #[test]
    fn subscriber_lifecycle_is_balanced() {
        let relay = relay();
        assert_eq!(relay.subscriber_count(), 0);
        assert_eq!(relay.generation(), 1);
        let (token, _rx) = relay.subscribe();
        assert_eq!(token.value(), 0);
        assert_eq!(relay.subscriber_count(), 1);
        relay.unsubscribe(token);
        assert_eq!(relay.subscriber_count(), 0);
    }

    /// 新订阅从当前窗口开始,首块带缺口标记,后续块连续且位置递增。
    #[test]
    fn new_subscriber_starts_with_gap_then_streams() {
        let relay = relay();
        let (token, mut rx) = relay.subscribe();
        relay.pump(&input(/*samples*/ 3000));
        relay.pump(&input(/*samples*/ 0));
        let chunks = drain(&mut rx);
        assert!(!chunks.is_empty(), "应收到样本块");
        assert!(
            chunks.first().is_some_and(|chunk| chunk.gap),
            "首块应标缺口"
        );
        assert!(
            chunks.iter().skip(1).all(|chunk| !chunk.gap),
            "后续块不应标缺口"
        );
        let total: usize = chunks.iter().map(|chunk| chunk.samples.len()).sum();
        assert_eq!(total, 3000);
        assert!(
            chunks
                .windows(2)
                .all(|pair| match (pair.first(), pair.get(1)) {
                    (Some(left), Some(right)) => right.position > left.position,
                    _ => false,
                }),
            "块位置应严格递增"
        );
        relay.unsubscribe(token);
    }

    /// 单块不超过 [`MAX_CHUNK`]。
    #[test]
    fn chunks_are_bounded() {
        let relay = relay();
        let (token, mut rx) = relay.subscribe();
        relay.pump(&input(/*samples*/ MAX_CHUNK * 2));
        let chunks = drain(&mut rx);
        assert!(
            chunks.iter().all(|chunk| chunk.samples.len() <= MAX_CHUNK),
            "单块不应超过 {MAX_CHUNK}"
        );
        relay.unsubscribe(token);
    }

    /// 慢订阅者不追赶旧样本:队列满后丢块,后续成功的块带缺口。
    #[test]
    fn slow_subscriber_skips_and_marks_gap() {
        let relay = relay();
        let (token, mut rx) = relay.subscribe();
        for _ in 0..SUBSCRIBER_QUEUE_CHUNKS.saturating_add(3) {
            relay.pump(&input(/*samples*/ MAX_CHUNK));
        }
        let queued = drain(&mut rx);
        assert_eq!(queued.len(), SUBSCRIBER_QUEUE_CHUNKS);
        // 丢块不会推进游标:恢复消费后的下一块从丢块前的位置继续并标缺口。
        relay.pump(&input(/*samples*/ MAX_CHUNK));
        let resumed = drain(&mut rx);
        assert!(
            resumed.first().is_some_and(|chunk| chunk.gap),
            "恢复后首块应标缺口"
        );
        relay.unsubscribe(token);
    }

    /// 切歌(轨道令牌变化)换代:窗口清空,订阅者下一块带缺口。
    #[test]
    fn track_change_bumps_generation() {
        let relay = relay();
        let (token, mut rx) = relay.subscribe();
        relay.pump(&input(/*samples*/ 100));
        let _ = drain(&mut rx);
        let before = relay.generation();
        let mut next = input(/*samples*/ 100);
        next.track_token = 2;
        relay.pump(&next);
        assert_eq!(relay.generation(), before.saturating_add(1));
        let chunks = drain(&mut rx);
        assert!(
            chunks
                .first()
                .is_some_and(|chunk| chunk.generation == before.saturating_add(1) && chunk.gap),
            "换代后的首块应带新代次与缺口"
        );
        relay.unsubscribe(token);
    }

    /// 采样率变化换代(0 = 未知不算变化)。
    #[test]
    fn sample_rate_change_bumps_generation() {
        let relay = relay();
        relay.pump(&input(/*samples*/ 10));
        let baseline = relay.generation();
        let mut same = input(/*samples*/ 10);
        same.sample_rate = 44_100;
        relay.pump(&same);
        assert_eq!(relay.generation(), baseline, "相同采样率不换代");
        let mut changed = input(/*samples*/ 10);
        changed.sample_rate = 48_000;
        relay.pump(&changed);
        assert_eq!(relay.generation(), baseline.saturating_add(1));
    }

    /// 上游丢样本(写入数超过已消费数)让下一块带缺口。
    #[test]
    fn upstream_drop_marks_gap() {
        let relay = relay();
        let (token, mut rx) = relay.subscribe();
        relay.pump(&input(/*samples*/ 100));
        let _ = drain(&mut rx);
        let mut lossy = input(/*samples*/ 100);
        lossy.pushed_total = 100 + 100 + 50;
        relay.pump(&lossy);
        let chunks = drain(&mut rx);
        assert!(
            chunks.first().is_some_and(|chunk| chunk.gap),
            "上游丢样本后应标缺口"
        );
        relay.unsubscribe(token);
    }

    /// 超过 1s 的位置跳变按 seek 换代。
    #[test]
    fn position_jump_bumps_generation() {
        let relay = relay();
        relay.pump(&input(/*samples*/ 44_100));
        let baseline = relay.generation();
        let mut jumped = input(/*samples*/ 100);
        jumped.position_ms = 60_000;
        relay.pump(&jumped);
        assert_eq!(relay.generation(), baseline.saturating_add(1));
    }

    /// 窗口只保留近 [`MAX_HISTORY`] 样本:迟到订阅者从窗口头开始。
    #[test]
    fn window_is_bounded() {
        let relay = relay();
        for _ in 0..((MAX_HISTORY / MAX_CHUNK).saturating_add(2)) {
            relay.pump(&input(/*samples*/ MAX_CHUNK));
        }
        let (token, mut rx) = relay.subscribe();
        relay.pump(&input(/*samples*/ 10));
        let chunks = drain(&mut rx);
        let total: usize = chunks.iter().map(|chunk| chunk.samples.len()).sum();
        assert!(total <= MAX_HISTORY.saturating_add(10), "窗口应有界");
        let first = chunks.first();
        assert!(first.is_some_and(|chunk| chunk.gap), "首块应标缺口");
        assert!(
            first.is_some_and(|chunk| chunk.position > 0),
            "迟到订阅者不应从 0 开始"
        );
        relay.unsubscribe(token);
    }
}

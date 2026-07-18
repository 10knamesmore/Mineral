//! Server 端 PCM 中继:把 [`SpectrumTap`] 收纳到 server 内部,client 通过 `pull` 拉。
//!
//! 解决「`SpectrumTap` 跨不了进程」的限制 —— 让 in-proc 和 connect 走同一接口。
//!
//! ## 实现:on-demand pull + 多游标 fan-out(无独立节奏)
//!
//! audio engine 内部的 SPSC ring 已有 4096 sample (~85ms @ 48kHz) 缓冲,远大于
//! client 30fps tick (33ms) 的间隔。中间再加一层 worker buffer 是冗余 —— 直接
//! `pull` 时 lock + `pop_into` 即可。
//!
//! 历史曾用 16ms tick worker drain 到 VecDeque,但那一层引入最坏 16ms 额外延迟,
//! spectrum 看着会「慢半拍」。on-demand pull 让最坏延迟 ≈ 一个 client tick gap (~5ms)。
//!
//! 多 client:tap 消费是破坏式的,直接共享会互抢样本。任一 `pull` 先把 ring 的新
//! 样本 drain 进短历史窗口([`FanOut`]),再按**调用方自己的游标**切片——drain 仍由
//! pull 触发、无独立节奏,上述低延迟语义原样保留;落后超出窗口的游标跳到最新
//! (频谱是瞬时可视化,补旧样本无意义)。

use std::collections::VecDeque;
use std::sync::Arc;

use mineral_audio::SpectrumTap;
use parking_lot::Mutex;
use rustc_hash::FxHashMap;

/// 历史窗口样本数上限(@48kHz ≈ 340ms):容纳最慢消费者一次 UI 卡顿的落后量,
/// 超出即跳最新。与 event hub capacity 同类的容量护栏,非行为旋钮。
const MAX_HISTORY: usize = 16 * 1024;

/// usize → u64(64 位平台恒成功;仅为绕过 `as` 禁用的显式转换)。
fn as_stream_pos(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

/// 多游标 fan-out 窗口:单写多读,每个读者持自己的绝对流位置游标。
/// 纯数据结构(不含 tap),样本注入由持有者驱动。
struct FanOut {
    /// 最近样本的尾部窗口(窗口尾 = 流位置 `total_written`)。
    history: VecDeque<f32>,

    /// 累计注入的总样本数(绝对流位置)。
    total_written: u64,

    /// 各连接已消费到的绝对流位置。
    cursors: FxHashMap<u64, u64>,
}

impl FanOut {
    /// 空窗口。
    fn new() -> Self {
        Self {
            history: VecDeque::new(),
            total_written: 0,
            cursors: FxHashMap::default(),
        }
    }

    /// 注入一批新样本,推进流位置并修剪窗口头部。
    fn ingest(&mut self, samples: &[f32]) {
        self.history.extend(samples.iter().copied());
        self.total_written += as_stream_pos(samples.len());
        let overflow = self.history.len().saturating_sub(MAX_HISTORY);
        if overflow > 0 {
            self.history.drain(..overflow);
        }
    }

    /// 按 `conn` 的游标取最多 N 个样本并推进游标。首拉从「最新 - n」起
    /// (与单消费者拉尾部窗口的行为一致);存量游标落后出窗口则跳到窗口起点。
    fn take(&mut self, conn: u64, n: usize) -> Vec<f32> {
        let window_start = self.total_written - as_stream_pos(self.history.len());
        let cursor = self
            .cursors
            .get(&conn)
            .copied()
            .unwrap_or_else(|| self.total_written.saturating_sub(as_stream_pos(n)))
            .max(window_start);
        let offset = usize::try_from(cursor - window_start).unwrap_or(usize::MAX);
        let take = n.min(self.history.len().saturating_sub(offset));
        let samples = self
            .history
            .iter()
            .skip(offset)
            .take(take)
            .copied()
            .collect::<Vec<f32>>();
        self.cursors.insert(conn, cursor + as_stream_pos(take));
        samples
    }

    /// 移除某连接的游标。
    fn remove(&mut self, conn: u64) {
        self.cursors.remove(&conn);
    }
}

/// tap + fan-out 的组合态(单把锁,pull 全程原子)。
struct Shared {
    /// SPSC consumer 单一持有者。
    tap: SpectrumTap,

    /// 多游标窗口。
    fan: FanOut,
}

/// PCM 中继。`Clone` 廉价(`Arc<Mutex>` 共享同一份状态)。
#[derive(Clone)]
pub(crate) struct PcmPuller {
    /// 共享态;Mutex 串行化 pull(样本量小,无争用热点)。
    state: Arc<Mutex<Shared>>,
}

impl PcmPuller {
    /// 接管 `SpectrumTap` 的 ownership。无后台 task。
    pub fn spawn(tap: SpectrumTap) -> Self {
        Self {
            state: Arc::new(Mutex::new(Shared {
                tap,
                fan: FanOut::new(),
            })),
        }
    }

    /// 按连接游标拉最多 N 个 sample(可能短于 N)+ 当前 sample_rate(0 = 没在播)。
    ///
    /// # Params:
    ///   - `conn`: 调用方连接 id(游标归属;首拉自动注册)
    ///   - `n`: 本次最多取的样本数
    pub fn pull(&self, conn: u64, n: usize) -> (Vec<f32>, u32) {
        let mut st = self.state.lock();
        let mut scratch = [0_f32; 1024];
        loop {
            let got = st.tap.pop_into(&mut scratch);
            if got == 0 {
                break;
            }
            let Some(batch) = scratch.get(..got) else {
                break;
            };
            st.fan.ingest(batch);
            if got < scratch.len() {
                break;
            }
        }
        let sr = st.tap.sample_rate();
        (st.fan.take(conn, n), sr)
    }

    /// 连接断开,移除其游标(serve 层连接收尾调;不清理会随断连累积泄漏)。
    pub fn drop_cursor(&self, conn: u64) {
        self.state.lock().fan.remove(conn);
    }
}

#[cfg(test)]
mod tests {
    use super::{FanOut, MAX_HISTORY};

    /// 顺序样本 `start..start+n`(値即序号,断言游标连续性用)。
    fn seq(start: usize, n: usize) -> Vec<f32> {
        (start..start + n)
            .map(|i| {
                // usize → f32 走 u16 中转(测试样本量远小于 u16::MAX,无损)。
                f32::from(u16::try_from(i).unwrap_or(u16::MAX))
            })
            .collect()
    }

    /// 双游标各自完整消费同一样本流,互不抢(单 consumer 直读时后拉者只能拿残余)。
    #[test]
    fn two_cursors_both_see_full_stream() {
        let mut fan = FanOut::new();
        fan.ingest(&seq(0, 100));
        let a1 = fan.take(/*conn*/ 1, /*n*/ 60);
        let b1 = fan.take(/*conn*/ 2, /*n*/ 60);
        assert_eq!(a1, seq(40, 60), "conn1 首拉:最近 60 个");
        assert_eq!(
            b1,
            seq(40, 60),
            "conn2 首拉同样拿最近 60 个,不受 conn1 影响"
        );
        fan.ingest(&seq(100, 50));
        let a2 = fan.take(/*conn*/ 1, /*n*/ 60);
        let b2 = fan.take(/*conn*/ 2, /*n*/ 60);
        assert_eq!(a2, seq(100, 50), "conn1 续读:游标衔接,恰是新注入的 50 个");
        assert_eq!(b2, seq(100, 50), "conn2 续读同上");
    }

    /// 落后超出窗口的游标跳到窗口起点(丢旧补不了,频谱语义正确)。
    #[test]
    fn lagging_cursor_jumps_to_window_start() {
        let mut fan = FanOut::new();
        fan.ingest(&seq(0, 10));
        let first = fan.take(/*conn*/ 1, /*n*/ 10);
        assert_eq!(first, seq(0, 10));
        // 注入超过窗口容量,conn1 的游标(=10)被挤出窗口。
        let big = vec![0.5_f32; MAX_HISTORY + 100];
        fan.ingest(&big);
        let resumed = fan.take(/*conn*/ 1, /*n*/ 4);
        assert_eq!(
            resumed,
            vec![0.5_f32; 4],
            "落后游标应跳到窗口起点继续,而不是空转或 panic"
        );
    }

    /// 游标移除后再拉视同新连接(从最近窗口起,不残留旧位置)。
    #[test]
    fn removed_cursor_restarts_fresh() {
        let mut fan = FanOut::new();
        fan.ingest(&seq(0, 100));
        let _ = fan.take(/*conn*/ 7, /*n*/ 100);
        fan.remove(/*conn*/ 7);
        let fresh = fan.take(/*conn*/ 7, /*n*/ 10);
        assert_eq!(fresh, seq(90, 10), "移除后首拉回到「最新 - n」起点");
    }
}

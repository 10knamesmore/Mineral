//! 引擎侧 2-slot 播放头:在「当前曲」之外多记一首「已预排进 rodio 队列的下一曲」,
//! 靠观测 sink 的 `sound_count`(队列里未耗尽的 source 数)递减来探测曲目边界,
//! 在边界处把 next 轮转成 cur,推进单调的 `finished_seq`(曲终计数)与 `track_token`
//! (当前曲身份令牌)。
//!
//! 纯状态机:不持有任何 rodio 句柄,只吃一个 `usize` 长度读数,故可脱离音频设备单测。
//! 真实 sink 的长度由引擎主循环每 tick 喂进 [`PlayHead::observe`]。

use std::sync::atomic::{AtomicU16, AtomicU64, Ordering};

/// 一个播放槽承载的引擎侧记账信息(decoder 已 append 进 rodio 队列后)。
#[derive(Clone, Copy, Default)]
pub(crate) struct Slot {
    /// 该流的代号(每次起播 / 预排 +1),下载侧据此过滤迟到信号。
    pub(crate) stream_gen: u64,

    /// decoder 探到的时长(ms,0 = 未知)。
    pub(crate) duration_ms: u64,

    /// 进度载体里属于本槽的下标(0/1),snapshot 读缓冲 / 完成据此定位。
    pub(crate) progress_idx: usize,

    /// 本槽是否有歌(= 是否「武装」等它自然播完)。
    pub(crate) occupied: bool,
}

/// 一次长度观测得出的边界分类。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Boundary {
    /// 无事发生(长度未降 / 无武装槽)。
    None,

    /// 当前曲自然播完、后面排着 next:已把 next 轮转成 cur(无缝续上)。
    Gapless,

    /// 当前曲自然播完、队列空:曲终,无续。
    EndOfQueue,
}

/// 引擎 2-slot 播放头,跨 tick 维护。`finished_seq` / `track_token` 写进 snapshot 供上层消费。
#[derive(Default)]
pub(crate) struct PlayHead {
    /// 正在出声的当前曲槽。
    pub(crate) cur: Slot,

    /// 已预排进 rodio 队列、等当前曲播完接续的下一曲槽。
    pub(crate) next: Slot,

    /// 单调递增的曲终计数:每次自然曲终(Gapless / EndOfQueue)+1。
    pub(crate) finished_seq: u64,

    /// 当前曲身份令牌:每次起播 / 边界轮转 +1。上层认它的**值变化**判定换曲。
    pub(crate) track_token: u64,
}

impl PlayHead {
    /// 起播一首新当前曲(cut-over):占用 cur、清空 next、令牌 +1。
    ///
    /// # Params:
    ///   - `slot`: 新当前曲槽(调用方已填好 stream_gen / duration_ms / progress_idx)。
    pub(crate) fn start(&mut self, slot: Slot) {
        self.cur = slot;
        self.next = Slot::default();
        self.track_token += 1;
    }

    /// 预排下一曲(AppendNext 实际 append 后调):占用 next 槽。
    ///
    /// # Params:
    ///   - `slot`: 下一曲槽。
    pub(crate) fn arm_next(&mut self, slot: Slot) {
        self.next = slot;
    }

    /// 按当前 cur / next 槽,从共享进度算出写进 snapshot 的 gapless 字段集。
    ///
    /// # Params:
    ///   - `progress`: 双槽共享进度载体。
    ///
    /// # Return:
    ///   该 tick 应写进 [`crate::snapshot::AudioSnapshot`] 的字段集。
    pub(crate) fn snapshot_fields(&self, progress: &SharedProgress) -> GaplessFields {
        let (buffered_bps, download_complete) = progress.read(&self.cur);
        let (next_buffered_bps, next_download_complete) = progress.read(&self.next);
        GaplessFields {
            current_track_token: self.track_token,
            track_finished_seq: self.finished_seq,
            duration_ms: if self.cur.occupied {
                self.cur.duration_ms
            } else {
                0
            },
            buffered_bps,
            download_complete,
            next_duration_ms: if self.next.occupied {
                self.next.duration_ms
            } else {
                0
            },
            next_buffered_bps,
            next_ready: self.next.occupied,
            next_download_complete,
        }
    }

    /// 观测 rodio sink 的当前 `sound_count`,推进状态机。
    ///
    /// 武装槽数(occupied 的 cur + next)即「我们认为还排在队列里的曲数」。观测到的 `len`
    /// 比它小,说明有一首自然耗尽:若 next 武装,则耗尽的是 cur、next 已接上 → 轮转;否则
    /// 队列已空 → 曲终。`len >= 武装数`(含未武装时退潮)一律视作无事,旧曲尾巴退潮不误判。
    ///
    /// # Params:
    ///   - `len`: 当前 sink 队列里未耗尽的 source 数。
    ///
    /// # Return:
    ///   本次观测得出的边界事件。
    pub(crate) fn observe(&mut self, len: usize) -> Boundary {
        let armed = usize::from(self.cur.occupied) + usize::from(self.next.occupied);
        if len >= armed {
            return Boundary::None;
        }
        self.finished_seq += 1;
        if self.next.occupied {
            // cur 自然播完,next 已无缝接上 → 轮转成新 cur,令牌 +1。
            self.cur = self.next;
            self.next = Slot::default();
            self.track_token += 1;
            Boundary::Gapless
        } else {
            // cur 播完且身后无歌 → 曲终,释放 cur。
            self.cur = Slot::default();
            Boundary::EndOfQueue
        }
    }
}

/// 单个槽的下载 / 缓冲进度原子组。下载侧(进度回调 / 完成 waiter)写,snapshot 读;
/// 代号字段与对应 [`Slot::stream_gen`] 比对,挡掉切歌后旧流的迟到信号。
#[derive(Default)]
pub(crate) struct ProgressSlot {
    /// capture 整段下完后 waiter store 的曲目代号(与槽 `stream_gen` 比对算下完)。
    pub(crate) done_gen: AtomicU64,

    /// `buffer_bps` 当前对应的流代号;回调写前先比对,旧流迟到回调直接 no-op。
    pub(crate) buffer_gen: AtomicU64,

    /// 当前流已缓冲比例(0..=10000 basis points)。
    pub(crate) buffer_bps: AtomicU16,
}

/// 双槽共享进度:`slots[0]` / `slots[1]` 分别承载 cur / next 当前占用的进度,
/// 具体哪格属于 cur 由 [`Slot::progress_idx`] 指明(边界轮转只翻下标,不搬数据)。
#[derive(Default)]
pub(crate) struct SharedProgress {
    /// 两个进度槽(固定 2 元,避开 HashMap 竞争)。
    slots: [ProgressSlot; 2],
}

impl SharedProgress {
    /// 取某槽的进度原子组引用(下标只会是 0/1;非 1 一律落 0 号槽,无 panic / 无 indexing)。
    ///
    /// # Params:
    ///   - `idx`: 进度槽下标(0/1)。
    pub(crate) fn slot(&self, idx: usize) -> &ProgressSlot {
        let [a, b] = &self.slots;
        if idx == 1 { b } else { a }
    }

    /// 读某播放槽的缓冲比例 + 是否下完(未占用槽恒 0 / false)。
    ///
    /// # Params:
    ///   - `slot`: 目标播放槽(读它的 `progress_idx` / `stream_gen`)。
    fn read(&self, slot: &Slot) -> (u16, bool) {
        if !slot.occupied {
            return (0, false);
        }
        let p = self.slot(slot.progress_idx);
        let bps = p.buffer_bps.load(Ordering::Acquire);
        let done = slot.stream_gen != 0 && p.done_gen.load(Ordering::Acquire) == slot.stream_gen;
        (bps, done)
    }
}

/// 一个 tick 由 [`PlayHead`] + [`SharedProgress`] 算出、写进 snapshot 的 gapless 字段集。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GaplessFields {
    /// 当前曲身份令牌。
    pub(crate) current_track_token: u64,

    /// 单调曲终计数。
    pub(crate) track_finished_seq: u64,

    /// 当前曲时长(ms;未占用为 0)。
    pub(crate) duration_ms: u64,

    /// 当前曲缓冲比例(basis points)。
    pub(crate) buffered_bps: u16,

    /// 当前曲远端是否下完。
    pub(crate) download_complete: bool,

    /// 下一曲时长(ms;未预排为 0)。
    pub(crate) next_duration_ms: u64,

    /// 下一曲缓冲比例(basis points)。
    pub(crate) next_buffered_bps: u16,

    /// 下一曲是否已预排到可无缝接续(= next 槽已占用)。
    pub(crate) next_ready: bool,

    /// 下一曲远端是否下完。
    pub(crate) next_download_complete: bool,
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{Boundary, GaplessFields, PlayHead, SharedProgress, Slot};

    /// 起播令牌从 0 → 1,cur 占用、next 空。
    #[test]
    fn start_bumps_token_and_occupies_cur() {
        let mut head = PlayHead::default();
        head.start(Slot {
            stream_gen: 1,
            duration_ms: 1000,
            progress_idx: 0,
            occupied: true,
        });
        assert_eq!(head.track_token, 1, "起播应令牌 +1");
        assert!(head.cur.occupied, "起播后 cur 应占用");
        assert!(!head.next.occupied, "起播后 next 应空");
        assert_eq!(head.cur.duration_ms, 1000);
    }

    /// 单曲自然播完、无 next:len 1→0 报 EndOfQueue,finished_seq +1,cur 释放。
    #[test]
    fn single_track_end_is_end_of_queue() {
        let mut head = PlayHead::default();
        head.start(Slot {
            stream_gen: 1,
            duration_ms: 1000,
            progress_idx: 0,
            occupied: true,
        });
        assert_eq!(head.observe(1), Boundary::None, "len 持平不应触发");
        assert_eq!(head.observe(0), Boundary::EndOfQueue, "len 降到 0 应曲终");
        assert_eq!(head.finished_seq, 1);
        assert!(!head.cur.occupied, "曲终后 cur 应释放");
    }

    /// 预排 next 后自然边界:len 2→1 报 Gapless,next 轮转成 cur,token / finished_seq 各 +1。
    #[test]
    fn gapless_boundary_rotates_next_into_cur() {
        let mut head = PlayHead::default();
        head.start(Slot {
            stream_gen: 1,
            duration_ms: 1000,
            progress_idx: 0,
            occupied: true,
        });
        head.observe(1); // 稳态
        head.arm_next(Slot {
            stream_gen: 2,
            duration_ms: 2000,
            progress_idx: 1,
            occupied: true,
        });
        let token_before = head.track_token;
        assert_eq!(head.observe(2), Boundary::None, "刚预排,len=2 持平不触发");
        assert_eq!(head.observe(1), Boundary::Gapless, "len 2→1 应无缝边界");
        assert_eq!(head.track_token, token_before + 1, "边界应令牌 +1");
        assert_eq!(head.finished_seq, 1, "边界应曲终 +1");
        assert_eq!(head.cur.stream_gen, 2, "next 应轮转成 cur");
        assert_eq!(head.cur.duration_ms, 2000);
        assert_eq!(head.cur.progress_idx, 1, "进度下标随轮转带过来");
        assert!(!head.next.occupied, "轮转后 next 应空");
    }

    /// 边界轮转后再自然播完(无新 next):应再报一次 EndOfQueue。
    #[test]
    fn after_rotation_next_end_is_end_of_queue() {
        let mut head = PlayHead::default();
        head.start(Slot {
            stream_gen: 1,
            duration_ms: 1000,
            progress_idx: 0,
            occupied: true,
        });
        head.arm_next(Slot {
            stream_gen: 2,
            duration_ms: 2000,
            progress_idx: 1,
            occupied: true,
        });
        head.observe(2);
        assert_eq!(head.observe(1), Boundary::Gapless);
        assert_eq!(
            head.observe(0),
            Boundary::EndOfQueue,
            "轮转后的 cur 播完应曲终"
        );
        assert_eq!(head.finished_seq, 2, "两次曲终累计");
    }

    /// snapshot_fields:cur / next 各从自己的 progress_idx 槽读缓冲;下完按各自 stream_gen 门控。
    #[test]
    fn snapshot_fields_reads_each_slot_by_its_index() {
        let mut head = PlayHead::default();
        head.start(Slot {
            stream_gen: 5,
            duration_ms: 1000,
            progress_idx: 0,
            occupied: true,
        });
        head.arm_next(Slot {
            stream_gen: 6,
            duration_ms: 2000,
            progress_idx: 1,
            occupied: true,
        });
        let progress = SharedProgress::default();
        // cur 在 0 号槽:缓冲 4000、已下完(done_gen == cur.stream_gen 5)。
        progress.slot(0).buffer_bps.store(4000, Ordering::Release);
        progress.slot(0).done_gen.store(5, Ordering::Release);
        // next 在 1 号槽:缓冲 8000、未下完(done_gen 0 ≠ 6)。
        progress.slot(1).buffer_bps.store(8000, Ordering::Release);

        let f = head.snapshot_fields(&progress);
        assert_eq!(
            f,
            GaplessFields {
                current_track_token: 1,
                track_finished_seq: 0,
                duration_ms: 1000,
                buffered_bps: 4000,
                download_complete: true,
                next_duration_ms: 2000,
                next_buffered_bps: 8000,
                next_ready: true,
                next_download_complete: false,
            }
        );
    }

    /// snapshot_fields:无 next 时 next_* 全零 / false;无 cur 时 duration / 缓冲归零。
    #[test]
    fn snapshot_fields_zero_for_unoccupied_slots() {
        let head = PlayHead::default(); // cur / next 均未占用
        let progress = SharedProgress::default();
        // 即便 0 号槽残留旧值,未占用也不该读出来。
        progress.slot(0).buffer_bps.store(9999, Ordering::Release);
        let f = head.snapshot_fields(&progress);
        assert_eq!(f.duration_ms, 0);
        assert_eq!(f.buffered_bps, 0, "未占用 cur 缓冲应归零");
        assert!(!f.download_complete);
        assert!(!f.next_ready, "未预排 next_ready 应 false");
        assert_eq!(f.next_buffered_bps, 0);
    }

    /// 用户主动 stop(把 cur 释放成未武装)后,sink 尾巴退潮 len→0 不应被误判曲终。
    #[test]
    fn disarmed_drain_is_not_a_boundary() {
        let mut head = PlayHead::default();
        head.start(Slot {
            stream_gen: 1,
            duration_ms: 1000,
            progress_idx: 0,
            occupied: true,
        });
        head.observe(1);
        // 模拟 Stop:释放 cur(不再武装)。
        head.cur.occupied = false;
        assert_eq!(
            head.observe(0),
            Boundary::None,
            "未武装时 len 退潮不应触发曲终"
        );
        assert_eq!(head.finished_seq, 0);
    }
}

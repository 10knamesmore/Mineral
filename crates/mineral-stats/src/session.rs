//! 收听会话的 gap 分段逻辑(纯函数,不碰 DB)。
//!
//! 播放活动(起播 / 自然接续 / resume)距上次活动超过 gap 阈值 → 关旧开新。会话 id 是
//! sessions 表的自增值,由 store 分配;本 tracker 只判「续旧还是开新」,并在 store 建好
//! 新会话后经 [`SessionTracker::begin`] 回填 id。daemon 重启后经 [`SessionTracker::resume`]
//! 种入上次会话,首播同样按 gap 判定(重启不必然断会话)。

/// 当前会话的内存状态。
#[derive(Clone, Copy, Debug)]
struct Active {
    /// sessions 表里的会话 id。
    id: i64,

    /// 上次活动时刻(epoch ms),滑动更新。
    last_activity_ms: i64,
}

/// 会话分段器。
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionTracker {
    /// 当前会话;`None` = 尚无会话(全新 / 从未活动)。
    current: Option<Active>,
}

/// 一次活动的分段判定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionDecision {
    /// 续用当前会话(store 应 `touch_session` 更新 ended_at)。
    Continue {
        /// 续用的会话 id。
        session_id: i64,
    },

    /// 应开新会话(store `open_session` 后调 [`SessionTracker::begin`] 回填)。
    StartNew,
}

impl SessionTracker {
    /// 用上次已知会话种入(daemon 重启接续):`last_activity_ms` 通常是该会话的
    /// `ended_at`,首播据此按 gap 判定续旧或开新。
    pub fn resume(id: i64, last_activity_ms: i64) -> Self {
        Self {
            current: Some(Active {
                id,
                last_activity_ms,
            }),
        }
    }

    /// 判定一次活动归属:距上次活动 ≤ gap 则续旧(并滑动更新 last_activity),否则
    /// 开新(不触碰旧会话状态,等 [`SessionTracker::begin`] 覆盖)。
    ///
    /// # Params:
    ///   - `at_ms`: 活动时刻 epoch ms
    ///   - `gap_ms`: 会话间隔阈值 ms
    pub fn on_activity(&mut self, at_ms: i64, gap_ms: i64) -> SessionDecision {
        match &mut self.current {
            Some(active) if at_ms - active.last_activity_ms <= gap_ms => {
                active.last_activity_ms = at_ms;
                SessionDecision::Continue {
                    session_id: active.id,
                }
            }
            _ => SessionDecision::StartNew,
        }
    }

    /// store 建好新会话后回填其 id 与起始时刻。
    pub fn begin(&mut self, id: i64, at_ms: i64) {
        self.current = Some(Active {
            id,
            last_activity_ms: at_ms,
        });
    }

    /// 当前会话 id(若有)。供 store 给事件行填 session_id。
    pub fn current_id(&self) -> Option<i64> {
        self.current.map(|a| a.id)
    }
}

#[cfg(test)]
mod tests {
    use super::{SessionDecision, SessionTracker};

    const GAP: i64 = 30 * 60 * 1000;

    #[test]
    fn fresh_tracker_starts_new() {
        let mut t = SessionTracker::default();
        assert_eq!(t.on_activity(1000, GAP), SessionDecision::StartNew);
        assert_eq!(t.current_id(), None, "StartNew 不自行分配 id");
    }

    #[test]
    fn within_gap_continues_same_session() {
        let mut t = SessionTracker::default();
        assert_eq!(t.on_activity(1000, GAP), SessionDecision::StartNew);
        t.begin(1, 1000);
        assert_eq!(
            t.on_activity(1000 + GAP, GAP),
            SessionDecision::Continue { session_id: 1 },
            "恰好等于 gap 仍续旧"
        );
    }

    #[test]
    fn beyond_gap_starts_new() {
        let mut t = SessionTracker::default();
        t.begin(1, 1000);
        assert_eq!(
            t.on_activity(1000 + GAP + 1, GAP),
            SessionDecision::StartNew
        );
    }

    #[test]
    fn continue_slides_the_window() {
        // 连续活动:每步只跟上次活动比 gap,不跟会话起点比。
        let mut t = SessionTracker::default();
        t.begin(1, 0);
        assert_eq!(
            t.on_activity(GAP, GAP),
            SessionDecision::Continue { session_id: 1 }
        );
        // 起点到此已 2*GAP,但距上次活动只 GAP → 仍续旧。
        assert_eq!(
            t.on_activity(2 * GAP, GAP),
            SessionDecision::Continue { session_id: 1 }
        );
    }

    #[test]
    fn resume_seeds_prior_session_then_gap_applies() {
        let mut t = SessionTracker::resume(9, 1000);
        assert_eq!(t.current_id(), Some(9));
        // 重启后首播在 gap 内 → 续上旧会话(重启不必然断会话)。
        assert_eq!(
            t.on_activity(1000 + GAP, GAP),
            SessionDecision::Continue { session_id: 9 }
        );
        // 远超 gap → 开新。
        assert_eq!(
            t.on_activity(1000 + 10 * GAP, GAP),
            SessionDecision::StartNew
        );
    }
}

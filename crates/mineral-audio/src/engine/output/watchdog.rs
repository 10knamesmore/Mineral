//! 输出回调停滞检测。

use std::time::{Duration, Instant};

/// 当前 rodio 队列是否应该持续收到 output callback。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OutputActivity {
    /// 当前有曲目且未暂停。
    Active,

    /// 暂停、停止或队列为空。
    Idle,
}

/// 一次 output callback 健康检查的输入。
#[derive(Clone, Copy, Debug)]
pub(super) struct WatchdogSample {
    /// 本次检查的单调时间。
    pub(super) observed_at: Instant,

    /// 当前播放活动状态。
    pub(super) activity: OutputActivity,

    /// output data callback 的当前调用序号。
    pub(super) callback_sequence: u64,
}

/// 通过 callback 序号判断系统输出 stream 是否停止回调。
pub(super) struct OutputWatchdog {
    /// active 状态下允许 callback 不变的最长时间。
    timeout: Duration,

    /// 上次观测到的 callback 序号。
    last_callback_sequence: Option<u64>,

    /// 当前连续无 callback 的起点；idle 或序号推进时清空。
    stalled_since: Option<Instant>,
}

impl OutputWatchdog {
    /// 构造停滞检测器。
    ///
    /// # Params:
    ///   - `timeout`: active 状态下 callback 连续不变多久后触发恢复。
    pub(super) fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            last_callback_sequence: None,
            stalled_since: None,
        }
    }

    /// 记录一次观测并判断当前是否应该重启 stream。
    ///
    /// 触发后把起点推进到本次观测时间，因此持续故障最多每个 `timeout` 重试一次。
    ///
    /// # Params:
    ///   - `sample`: 当前活动状态、callback 序号与单调时间。
    ///
    /// # Return:
    ///   仅当 active callback 连续停滞达到阈值时为 `true`。
    pub(super) fn should_restart(&mut self, sample: WatchdogSample) -> bool {
        let callback_advanced = self.last_callback_sequence != Some(sample.callback_sequence);
        self.last_callback_sequence = Some(sample.callback_sequence);

        if sample.activity == OutputActivity::Idle || callback_advanced {
            self.stalled_since = None;
            return false;
        }

        let Some(stalled_since) = self.stalled_since else {
            self.stalled_since = Some(sample.observed_at);
            return false;
        };
        if sample.observed_at.saturating_duration_since(stalled_since) < self.timeout {
            return false;
        }

        self.stalled_since = Some(sample.observed_at);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{OutputActivity, OutputWatchdog, WatchdogSample};

    #[test]
    fn watchdog_restarts_only_an_active_stream_without_callbacks() {
        let timeout = Duration::from_secs(/*secs*/ 1);
        let started_at = Instant::now();
        let mut watchdog = OutputWatchdog::new(timeout);

        assert!(!watchdog.should_restart(WatchdogSample {
            observed_at: started_at,
            activity: OutputActivity::Idle,
            callback_sequence: 7,
        }));
        assert!(!watchdog.should_restart(WatchdogSample {
            observed_at: started_at + timeout,
            activity: OutputActivity::Active,
            callback_sequence: 7,
        }));
        assert!(!watchdog.should_restart(WatchdogSample {
            observed_at: started_at + timeout + timeout - Duration::from_millis(/*millis*/ 1),
            activity: OutputActivity::Active,
            callback_sequence: 7,
        }));
        assert!(watchdog.should_restart(WatchdogSample {
            observed_at: started_at + timeout + timeout,
            activity: OutputActivity::Active,
            callback_sequence: 7,
        }));
        assert!(!watchdog.should_restart(WatchdogSample {
            observed_at: started_at + timeout + timeout + timeout,
            activity: OutputActivity::Active,
            callback_sequence: 8,
        }));
    }
}

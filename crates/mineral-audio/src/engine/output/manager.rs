//! Replaces CPAL streams after successful device selection while retaining playback state.

use std::sync::Arc;
use std::time::{Duration, Instant};

use mineral_common::revision::Revision;

use super::devices;
use super::stream::DeviceStream;
use super::watchdog::{OutputActivity, OutputWatchdog, WatchdogSample};
use crate::engine::render::SharedPlayback;
use crate::{AudioOutput, OutputDevice, OutputTarget};

/// 系统默认路由检查间隔与回调停滞超时。
const DEVICE_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// 引擎持有的输出流；流代次决定哪个设备回调可以消费 PCM。
pub(crate) struct Output {
    /// 实际系统输出；失去设备后为空。
    stream: Option<DeviceStream>,

    /// Playback and prepared PCM survive replacement of the device stream.
    playback: Arc<SharedPlayback>,

    /// Only callbacks from the committed stream may consume PCM.
    generation: Revision,

    /// 用户选择的设备策略，不因设备断开而改成其他设备。
    target: OutputTarget,

    /// 上一次观察到的系统默认设备，用于检测路由改变。
    system_default_id: Option<String>,

    /// 最近一次路由检查。
    last_device_check: Instant,

    /// 已打开流的回调停滞检测。
    watchdog: OutputWatchdog,
}

impl Output {
    /// 尝试打开系统默认设备；没有设备时保留控制面等待后续选择。
    pub(crate) fn open(playback: Arc<SharedPlayback>) -> Self {
        let mut output = Self {
            stream: None,
            playback,
            generation: Revision::default(),
            target: OutputTarget::SystemDefault,
            system_default_id: devices::default_id(),
            last_device_check: Instant::now(),
            watchdog: OutputWatchdog::new(DEVICE_CHECK_INTERVAL),
        };
        if let Err(error) = output.select(OutputTarget::SystemDefault) {
            mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "no output device; waiting for an available output");
        }
        output
    }

    /// 读取已成功打开且未报告失效的输出配置。
    pub(crate) fn info(&self) -> Option<&Arc<AudioOutput>> {
        self.stream
            .as_ref()
            .filter(|stream| !stream.failed())
            .map(|stream| &stream.info)
    }

    /// 枚举 daemon 机器上的输出设备。
    pub(crate) fn devices() -> crate::Result<Vec<OutputDevice>> {
        devices::list()
    }

    /// 目标流启动成功后才替换当前流；失败时保留当前输出和播放位置。
    pub(crate) fn select(&mut self, target: OutputTarget) -> crate::Result<()> {
        mineral_log::info!(target: "audio", selection = ?target, "opening audio output");
        let generation = self.generation.next();
        let stream = DeviceStream::open(target.clone(), Arc::clone(&self.playback), generation)?;
        mineral_log::info!(target: "audio", device_id = %stream.info.device_id, device_name = %stream.info.device_name, sample_rate_hz = stream.info.sample_rate_hz, channels = stream.info.channels, sample_format = %stream.info.sample_format, "audio output selected");
        self.playback.configure(stream.format, generation)?;
        self.generation = generation;
        self.stream = Some(stream);
        self.target = target;
        self.system_default_id = devices::default_id();
        self.watchdog = OutputWatchdog::new(DEVICE_CHECK_INTERVAL);
        Ok(())
    }

    /// 失效流暂停并释放；检测默认路由变化后跟随默认策略或重新绑定指定设备。
    pub(crate) fn refresh_device(&mut self) {
        if self.stream.as_ref().is_some_and(DeviceStream::failed) {
            mineral_log::warn!(target: "audio", selection = ?self.target, "audio output lost; pausing playback");
            self.playback.state.lock().paused = true;
            self.stream = None;
        }
        if self.last_device_check.elapsed() < DEVICE_CHECK_INTERVAL {
            return;
        }
        self.last_device_check = Instant::now();
        let default_id = devices::default_id();
        let previous_default = std::mem::replace(&mut self.system_default_id, default_id.clone());
        let changed = default_id != previous_default;
        let reopen = match &self.target {
            OutputTarget::SystemDefault => {
                default_id.is_some() && (changed || self.stream.is_none())
            }
            // CoreAudio 对当时默认设备建立的流可能跟随系统路由；改成固定设备后重新绑定。
            OutputTarget::Device(id) => {
                changed && self.stream.is_some() && previous_default.as_ref() == Some(id)
            }
        };
        if reopen {
            if let Err(error) = self.select(self.target.clone()) {
                mineral_log::warn!(target: "audio", selection = ?self.target, error = mineral_log::chain(&error), "audio output route update failed; pausing playback");
                self.playback.state.lock().paused = true;
                self.stream = None;
            }
        } else if self.target == OutputTarget::SystemDefault
            && default_id.is_none()
            && self.stream.is_some()
        {
            self.playback.state.lock().paused = true;
            self.stream = None;
            mineral_log::warn!(target: "audio", "system default output disappeared; pausing playback");
        }
    }

    /// 播放期间回调持续停滞达到超时后，重启同一条系统流。
    pub(crate) fn recover_if_stalled(&mut self, playing: bool, position_ms: u64) {
        let Some(stream) = &self.stream else {
            return;
        };
        let sequence = stream.callback_sequence();
        if !self.watchdog.should_restart(WatchdogSample {
            observed_at: Instant::now(),
            activity: if playing {
                OutputActivity::Active
            } else {
                OutputActivity::Idle
            },
            callback_sequence: sequence,
        }) {
            return;
        }
        mineral_log::warn!(target: "audio", position_ms, callback_sequence = ?sequence, "audio output callback stalled; restarting stream");
        match stream.restart() {
            Ok(()) => {
                mineral_log::info!(target: "audio", position_ms, "audio output stream restarted")
            }
            Err(error) => {
                mineral_log::warn!(target: "audio", error = mineral_log::chain(&error), "audio output stream restart failed")
            }
        }
    }
}

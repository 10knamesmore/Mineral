//! 进程内音频引擎:消费已打开媒体并负责 decode、输出、gapless 与 PCM tap。
//!
//! 引擎跑在专属 OS 线程,owns rodio `OutputStream` 与 `Sink`;mpsc 命令通道把 UI 操作
//! 转给 worker。snapshot 用 `Arc<Mutex<_>>` 共享给 UI;每次写入经
//! [`AudioHandle::snapshot_changes`] 发信号,订阅方据此即时推送,不必定频轮询。

mod bps;
mod command;
mod engine;
mod envelope;
mod handle;
mod queue_slots;
mod snapshot;
mod tap;

pub use bps::Bps;
pub use envelope::{
    ENVELOPE_VERSION, EnvelopeParams, HighpassParams, ShelfParams, envelope_from_file,
    envelope_from_samples,
};
pub use handle::{AudioHandle, AudioMode, EngineParams, SpectrumTap};
pub use snapshot::{AudioBackend, AudioSnapshot};

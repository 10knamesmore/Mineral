//! 进程内音频引擎:rodio + stream-download 封装,UI 通过 [`AudioHandle`] 调命令、拉 [`AudioSnapshot`]。
//!
//! 引擎跑在专属 OS 线程,owns rodio `OutputStream` 与 `Sink`;mpsc 命令通道把 UI 操作
//! 转给 worker。snapshot 用 `Arc<Mutex<_>>` 共享给 UI 周期 polling。

mod command;
mod engine;
mod handle;
mod snapshot;
mod tap;

pub use handle::{AudioHandle, SpectrumTap};
pub use snapshot::AudioSnapshot;

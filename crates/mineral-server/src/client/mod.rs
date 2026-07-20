//! Client 调用面:抽象契约与其同进程实现。

mod contract;
mod handle;
mod wire;

pub use contract::Client;
pub use handle::ClientHandle;

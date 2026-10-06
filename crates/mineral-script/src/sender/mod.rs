//! 向 daemon 脚本线程投递音乐请求与回执;与配置加载无关。

mod handle;

pub use handle::ScriptSender;

#[cfg(test)]
mod tests;

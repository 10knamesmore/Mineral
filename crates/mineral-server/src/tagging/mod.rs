//! 落盘歌曲的内嵌 metadata 打标：异步采集并写入 tag，失败只记日志，不影响下载与播放。

mod assemble;
mod queue;
mod worker;
mod write;

pub(crate) use queue::TaggingQueue;

#[cfg(test)]
mod tests;

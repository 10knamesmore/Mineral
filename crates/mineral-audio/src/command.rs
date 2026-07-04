//! UI → engine 的命令枚举。

use std::path::PathBuf;

use mineral_model::{MediaUrl, StreamLayout};

/// 投递给 engine 主循环的一条指令。
pub(crate) enum AudioCommand {
    /// 切到这个 URL,从头播。
    ///
    /// `capture` 非空时(仅对 `Remote`),边播边把下载的字节落到该路径,供播完后入缓存;
    /// `Local` 或 `capture` 为空时不落盘,维持原行为。
    Play {
        /// 播放源。
        url: MediaUrl,

        /// 取流附加请求头(如 B站 baseUrl 播放需 `Referer`);空 = 无附加头,走默认无头 client。
        headers: Vec<(String, String)>,

        /// 捕获落盘路径(`Remote` + 想缓存时给)。
        capture: Option<PathBuf>,

        /// 流的容器布局:决定解码器 seekable / 流式打开(分片远端流流式,避免 open 预扫全片)。
        layout: StreamLayout,
    },
    /// 预排下一曲:在当前曲播完前把它的 decoder 排进 rodio 队列,实现无缝接续。
    ///
    /// 与 [`Self::Play`] 的区别:**不**打断当前曲、**不** `stop()`/`play()`,只 `append`。
    /// 远端源的建流 / 预缓冲在引擎 runtime 上**链下**进行,就绪后才 append,避免阻塞命令线程。
    /// `capture` 语义同 [`Self::Play`](仅 `Remote` 落盘供入缓存)。
    AppendNext {
        /// 下一曲播放源。
        url: MediaUrl,

        /// 取流附加请求头(如 B站 baseUrl 播放需 `Referer`);空 = 无附加头,走默认无头 client。
        headers: Vec<(String, String)>,

        /// 捕获落盘路径(`Remote` + 想缓存时给)。
        capture: Option<PathBuf>,

        /// 流的容器布局:决定解码器 seekable / 流式打开(分片远端流流式,避免 open 预扫全片)。
        layout: StreamLayout,
    },
    /// 撤销「尚未 append 进队列」的待建下一曲(缓冲不及预期时的回退;已 append 则无效)。
    ClearNext,
    /// 暂停当前曲目。
    Pause,
    /// 从暂停态恢复。
    Resume,
    /// 停掉当前曲目并清空 sink。
    Stop,
    /// 设置音量(0..=100)。
    SetVolume(u8),
    // seek 不走 channel,走 [`crate::handle::AudioHandle`] 的 `Arc<Mutex<Option<Duration>>>`
    // mailbox(latest-wins),engine 主循环每 tick `take()` 一次 —— 长按 ←/→ 时合并。
}

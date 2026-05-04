//! [`Server`]:audio engine + task scheduler + channels 的单进程收纳容器。

use std::sync::Arc;

use mineral_audio::{AudioHandle, SpectrumTap};
use mineral_channel_core::MusicChannel;
use mineral_task::{ChannelFetchKind, Priority, Scheduler, TaskKind};

use crate::client::ClientHandle;

/// 后台 server。`spawn` 启动 audio engine 线程 + scheduler worker,投递初始任务,
/// 再把这些 handle 收纳起来对外发 [`ClientHandle`]。
pub struct Server {
    /// audio engine 句柄(自身已是 Arc + channel-based,clone 廉价)。
    audio: AudioHandle,

    /// task scheduler(同上,Arc + worker pool)。
    scheduler: Scheduler,

    /// PCM tap:SPSC consumer,唯一所有权。`take_spectrum_tap` 取走后变 None。
    spectrum_tap: Option<SpectrumTap>,

    /// 注册到 server 的全部音乐源 handle。Scheduler 内部已经 clone 一份用于
    /// lane 路由,这里再持一份是为了未来给 client 列「有哪些 source 可选」之类
    /// 的元信息(以及让 `Server` 显式 own 入参)。
    #[allow(dead_code, reason = "字段当前未读,后续 channel 元信息 API 会用到")]
    channels: Vec<Arc<dyn MusicChannel>>,
}

impl Server {
    /// 启动 audio engine + scheduler,并按 channel 列表投递「初始拉数据」的任务
    /// (每个 channel 一个 `MyPlaylists` + 一个 `LikedSongIds`)。
    ///
    /// # Params:
    ///   - `channels`: 已构造好的全部音乐源 handle。空 vec 也合法(纯 UI 演示)。
    ///
    /// # Return:
    ///   audio engine 启动失败 / 默认输出设备不可用时返回 `Err`。
    pub fn spawn(channels: Vec<Arc<dyn MusicChannel>>) -> color_eyre::Result<Self> {
        let scheduler = Scheduler::new(&channels);
        submit_initial_loads(&scheduler, &channels);
        let (audio, spectrum_tap) = AudioHandle::spawn()?;
        Ok(Self {
            audio,
            scheduler,
            spectrum_tap: Some(spectrum_tap),
            channels,
        })
    }

    /// 取走 PCM tap;只能取一次,再次调用返回 `None`。
    ///
    /// SPSC consumer 不能 clone,所以 tap 必须由唯一持有者(当前是 TUI 的 spectrum
    /// 渲染)拿走。未来 IPC 化时这条会被「server 推 PCM 流」替换。
    pub fn take_spectrum_tap(&mut self) -> Option<SpectrumTap> {
        self.spectrum_tap.take()
    }

    /// 拿一个 client handle。clone 廉价(内部都是 Arc),可任意复制给多处调用。
    pub fn client(&self) -> ClientHandle {
        ClientHandle::new(self.audio.clone(), self.scheduler.clone())
    }

    /// 显式 shutdown。当前实现就是 drop 自身,利用 [`AudioHandle`] / [`Scheduler`]
    /// 现有的 Drop 链(命令通道 close → engine 线程退出 / worker 线程感知)。
    /// 未来如要严格控制顺序(比如先 stop 当前播放再退 engine),在这里加显式
    /// 命令再 drop。
    pub fn shutdown(self) {
        drop(self);
    }
}

fn submit_initial_loads(scheduler: &Scheduler, channels: &[Arc<dyn MusicChannel>]) {
    for ch in channels {
        let source = ch.source();
        scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::MyPlaylists { source }),
            Priority::User,
        );
        // user-data 类是装饰,不阻塞用户 navigate,走 Background。
        // channel 不支持(NotSupported)就静默失败一次,后续不重试。
        scheduler.submit(
            TaskKind::ChannelFetch(ChannelFetchKind::LikedSongIds { source }),
            Priority::Background,
        );
    }
}

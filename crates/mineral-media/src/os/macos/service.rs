//! macOS MediaService:对外同步 API,内部把状态更新投递给专属线程应用到系统媒体
//! 中心;命令经 `on_command` 回调(由主线程 run loop 派发触发)回传。
//!
//! 专属线程独占所有 Objective-C 对象(`nowPlayingInfo` 字典等不可跨线程),故
//! [`MediaService`] 只持 channel 发送端,保持 `Send` + `Clone`。

use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::Duration;

use color_eyre::eyre::eyre;

use super::command::register_commands;
use super::convert::secs;
use super::now_playing::MacNowPlaying;
use crate::command::{LoopMode, MediaCommand};
use crate::config::MediaConfig;
use crate::state::{NowPlaying, PlaybackState};

/// 主线程 / tokio → 专属线程的状态更新消息。
enum Update {
    /// 换歌 / 元数据变化:重设当前曲目。
    Metadata(NowPlaying),

    /// 播放态 + 进度。
    Playback {
        /// 播放 / 暂停 / 停止。
        state: PlaybackState,

        /// 当前进度;`None` 表示不更新位置。
        position: Option<Duration>,
    },

    /// 非线性位置跳变(seek):重设进度基准。
    Seeked(Duration),

    /// 当前曲目封面的编码图片字节。
    Artwork(Vec<u8>),
}

/// 系统媒体服务句柄(macOS = 系统 Now Playing)。
#[derive(Clone)]
pub struct MediaService {
    /// 向专属线程投递状态更新。
    tx: Sender<Update>,
}

impl MediaService {
    /// 起专属线程:注册系统媒体命令(接到 `on_command`)+ 消费状态更新。
    ///
    /// # Params:
    ///   - `config`: macOS 后端不使用(无总线名 / identity 概念)。
    ///   - `on_command`: 收到系统媒体控件命令时回调,由主线程 run loop 派发触发。
    ///
    /// # Return:
    ///   线程起不来时返回 `Err`。
    pub fn spawn(
        config: &MediaConfig,
        on_command: Arc<dyn Fn(MediaCommand) + Send + Sync>,
    ) -> color_eyre::Result<Self> {
        let _ = config;
        let (tx, rx) = channel::<Update>();
        thread::Builder::new()
            .name("mineral-nowplaying".to_owned())
            .spawn(move || run_thread(&rx, &on_command))
            .map_err(|e| eyre!("spawn now-playing thread: {e}"))?;
        Ok(Self { tx })
    }

    /// 上报当前曲目元数据。
    pub fn set_now_playing(&self, now_playing: &NowPlaying) -> color_eyre::Result<()> {
        self.send(Update::Metadata(now_playing.clone()))
    }

    /// 上报播放状态与进度。
    pub fn set_playback(
        &self,
        state: PlaybackState,
        position: Option<Duration>,
    ) -> color_eyre::Result<()> {
        self.send(Update::Playback { state, position })
    }

    /// 通知非线性位置跳变(seek),重设系统进度条基准。
    pub fn notify_seek(&self, position: Duration) -> color_eyre::Result<()> {
        self.send(Update::Seeked(position))
    }

    /// 上报随机播放开关。macOS 系统媒体中心无随机态展示面,本平台 no-op。
    pub fn set_shuffle(&self, shuffle: bool) -> color_eyre::Result<()> {
        let _ = shuffle;
        Ok(())
    }

    /// 上报循环模式。macOS 系统媒体中心无循环态展示面,本平台 no-op。
    pub fn set_loop(&self, mode: LoopMode) -> color_eyre::Result<()> {
        let _ = mode;
        Ok(())
    }

    /// 设置当前曲目封面(已编码的图片字节,由上层拉取后传入)。
    pub fn set_artwork(&self, image_bytes: &[u8]) -> color_eyre::Result<()> {
        self.send(Update::Artwork(image_bytes.to_vec()))
    }

    /// 投递一条更新;专属线程已退出则返回 `Err`。
    fn send(&self, update: Update) -> color_eyre::Result<()> {
        self.tx
            .send(update)
            .map_err(|e| eyre!("now-playing thread gone: {e}"))
    }
}

/// 专属线程主体:注册命令,然后阻塞消费状态更新直到发送端全部释放。
fn run_thread(rx: &Receiver<Update>, on_command: &Arc<dyn Fn(MediaCommand) + Send + Sync>) {
    register_commands(on_command);
    let mut state = MacNowPlaying::new();
    while let Ok(update) = rx.recv() {
        match update {
            Update::Metadata(now_playing) => state.set_metadata(&now_playing),
            Update::Playback { state: s, position } => {
                state.set_playback(s, position.map(secs));
            }
            Update::Seeked(position) => state.seeked(secs(position)),
            Update::Artwork(bytes) => state.set_artwork(&bytes),
        }
    }
}

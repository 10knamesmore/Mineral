//! 图片通过 channel 报告上传与释放；已上传图片表由 TUI 线程独占。

use std::sync::mpsc::{self, Receiver, Sender};

use rustc_hash::FxHashMap;

use super::command::delete_image;
use crate::image::graphics::TerminalRelay;

pub(crate) enum KittyImageEvent {
    Uploaded {
        id: u32,

        /// 删除图片时使用与上传相同的传输方式。
        relay: TerminalRelay,
    },

    Released(u32),
}

/// 缓存与转场通过 Arc 共享此引用；最后一个引用释放时只发事件，不执行终端 I/O。
pub(crate) struct KittyImageHandle {
    id: u32,

    events: Sender<KittyImageEvent>,
}

impl KittyImageHandle {
    pub(super) fn new(id: u32, events: Sender<KittyImageEvent>) -> Self {
        Self { id, events }
    }

    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    pub(super) fn uploaded(&self, relay: TerminalRelay) {
        let _ = self
            .events
            .send(KittyImageEvent::Uploaded { id: self.id, relay });
    }
}

impl Drop for KittyImageHandle {
    fn drop(&mut self) {
        let _ = self.events.send(KittyImageEvent::Released(self.id));
    }
}

/// TUI 独占的本会话图片表，帧提交时消费事件，正常退出时清空。
pub(crate) struct KittyImageRegistry {
    events: Receiver<KittyImageEvent>,

    /// 已上传图片及其控制序列传输方式，用于释放和退出时删除。
    uploaded: FxHashMap<u32, TerminalRelay>,
}

impl KittyImageRegistry {
    pub(crate) fn new() -> (Self, Sender<KittyImageEvent>) {
        let (events, receiver) = mpsc::channel();
        (
            Self {
                events: receiver,
                uploaded: FxHashMap::default(),
            },
            events,
        )
    }

    /// 只消费生命周期事件，不扫描存活图片；命令在新帧 cell 更新后提交。
    pub(crate) fn take_pending_deletions(&mut self) -> String {
        let mut commands = String::new();
        while let Ok(event) = self.events.try_recv() {
            match event {
                KittyImageEvent::Uploaded { id, relay } => {
                    self.uploaded.insert(id, relay);
                }
                KittyImageEvent::Released(id) => {
                    if let Some(relay) = self.uploaded.remove(&id) {
                        commands.push_str(&delete_image(id, relay));
                    }
                }
            }
        }
        commands
    }

    /// 包含尚未消费的事件以及缓存、转场仍持有的图片。
    pub(crate) fn take_all_deletions(&mut self) -> String {
        let mut commands = self.take_pending_deletions();
        for (id, relay) in self.uploaded.drain() {
            commands.push_str(&delete_image(id, relay));
        }
        commands
    }
}

//! queue 浮层发起的队列结构编辑:定位构造、请求发送、回执处理。
//!
//! 队列是后端权威态,这里**不本地预改**队列内容——新样子随后由 server 推送带来。
//! 本地抢跑会在编辑被拒时留下一个服务端并不存在的画面。

use mineral_protocol::{QueueAnchor, QueueEditOutcome, QueueOp, QueuePos};

use super::App;
use crate::components::popup::OverlayAction;
use crate::components::toast::notifications::{TextTint, tinted_text_item};

impl App {
    /// 处理 queue 浮层产出的编辑类动作。
    ///
    /// # Params:
    ///   - `action`: 浮层动作(仅队列编辑族;其余变体在此为 no-op)
    pub(super) fn run_queue_overlay_action(&mut self, action: &OverlayAction) {
        match *action {
            OverlayAction::QueueActionMenu { idx, anchor } => {
                self.open_queue_action_menu(idx, anchor);
            }
            OverlayAction::ToggleLoveQueueIndex(idx) => self.toggle_love_queue_index(idx),
            OverlayAction::DownloadQueueIndex(idx) => self.download_queue_index(idx),
            OverlayAction::ReorderQueueIndex { idx, down } => {
                let to = if down { QueuePos::Down } else { QueuePos::Up };
                self.send_queue_edit(idx, |at| QueueOp::Move { at, to });
            }
            _ => {}
        }
    }

    /// 切换队列第 `idx` 项的收藏态(乐观翻转 + 转发)。
    ///
    /// 不复用 browse 页那条路径:那条按「当前 View 的选中曲」取歌,而浮层自持光标。
    fn toggle_love_queue_index(&mut self, idx: usize) {
        let Some(song) = self.state.player.queue.get(idx).cloned() else {
            return;
        };
        self.client.toggle_love(song.clone());
        self.state.toggle_loved_local(&song);
    }

    /// 下载队列第 `idx` 项。
    fn download_queue_index(&mut self, idx: usize) {
        let Some(song) = self.state.player.queue.get(idx).cloned() else {
            return;
        };
        self.client
            .download(mineral_protocol::DownloadTarget::Song(Box::new(song)));
    }

    /// 用队列第 `idx` 项的身份构造定位,交给 `build` 拼出操作后送 server。
    ///
    /// 定位带身份是防多 client 错位:另一个 client 刚改过队列时,同一个下标已经是别的歌,
    /// server 据身份判出过期并拒绝,而不是删掉用户没打算删的那首。
    ///
    /// # Params:
    ///   - `idx`: 目标条目下标
    ///   - `build`: 拿定位拼出具体操作
    pub(crate) fn send_queue_edit(
        &mut self,
        idx: usize,
        build: impl FnOnce(QueueAnchor) -> QueueOp,
    ) {
        let Some(song) = self.state.player.queue.get(idx) else {
            return;
        };
        let anchor = QueueAnchor::new(idx, song.id.clone());
        self.apply_queue_edit(build(anchor));
    }

    /// 送一次队列编辑并按回执提示。
    ///
    /// # Params:
    ///   - `op`: 待执行的操作
    pub(crate) fn apply_queue_edit(&mut self, op: QueueOp) {
        match self.client.queue_edit(op) {
            // 不自动重试:重试会在用户看不见的情况下作用到另一首歌上。
            QueueEditOutcome::Stale => {
                self.notifications.flash(tinted_text_item(
                    "queue changed elsewhere, nothing done".to_owned(),
                    TextTint::Error,
                ));
            }
            QueueEditOutcome::Applied | QueueEditOutcome::NoOp => {}
        }
    }
}

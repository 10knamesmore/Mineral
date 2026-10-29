//! 串行提交队列编辑；连续移动只保留最新待执行方向，等回执和权威队列同步后再定位。

use std::collections::VecDeque;

use mineral_client::operation::Outcome;
use mineral_common::revision::Revision;
use mineral_protocol::{QueueAnchor, QueueEditOutcome, QueueOp, QueuePos};

use super::App;
use crate::components::popup::OverlayAction;

/// 所有本地队列编辑共用一个在途请求，移动方向不提前转换成旧队列上的定位。
#[derive(Default)]
pub(super) struct QueueEdits {
    pending: Option<PendingQueueEdit>,

    queued_operations: VecDeque<QueueOp>,

    reorder: Option<QueueReorder>,
}

struct PendingQueueEdit {
    /// 回执到达后保存提交版本；同步达到该版本前不发送下一笔。
    queue_version: Option<Revision>,

    /// 仅连续移动跟随光标；用户导航后清除，不改变已经提交的请求。
    destination: Option<usize>,
}

/// 连续移动以已确认版本中的下标跟随条目，不按歌曲身份查找重复项。
struct QueueReorder {
    anchor: QueueAnchor,

    queue_version: Option<Revision>,

    /// 在途请求之外最多保留一步，新输入覆盖它，避免松手后继续执行积压移动。
    next_direction: Option<ReorderDirection>,
}

#[derive(Clone, Copy)]
enum ReorderDirection {
    Up,

    Down,
}

impl ReorderDirection {
    fn position(self) -> QueuePos {
        match self {
            Self::Up => QueuePos::Up,
            Self::Down => QueuePos::Down,
        }
    }

    fn destination(self, index: usize, len: usize) -> Option<usize> {
        match self {
            Self::Up => index.checked_sub(1),
            Self::Down => index.checked_add(1).filter(|&next| next < len),
        }
    }
}

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
                let direction = if down {
                    ReorderDirection::Down
                } else {
                    ReorderDirection::Up
                };
                self.request_queue_reorder(idx, direction);
            }
            _ => {}
        }
    }

    /// 切换队列第 `idx` 项的收藏态(乐观翻转 + 转发)。
    ///
    /// 不复用 browse 页那条路径:那条按「当前 View 的选中曲」取歌,而浮层自持光标。
    fn toggle_love_queue_index(&mut self, idx: usize) {
        let Some(song) = self.state.models.player.queue.get(idx).cloned() else {
            return;
        };
        self.client.toggle_love(song.clone());
        self.state.toggle_loved_local(&song);
    }

    /// 下载队列第 `idx` 项。
    fn download_queue_index(&mut self, idx: usize) {
        let Some(song) = self.state.models.player.queue.get(idx).cloned() else {
            return;
        };
        self.client
            .download(mineral_protocol::DownloadTarget::Song(Box::new(song)));
    }

    fn request_queue_reorder(&mut self, idx: usize, direction: ReorderDirection) {
        if self.queue_edits.reorder.is_none() {
            let player = &self.state.models.player;
            let Some(song) = player.queue.get(idx) else {
                return;
            };
            self.queue_edits.reorder = Some(QueueReorder {
                anchor: QueueAnchor::new(idx, song.id.clone()),
                queue_version: player.versions.queue,
                next_direction: None,
            });
        }
        if let Some(reorder) = &mut self.queue_edits.reorder {
            reorder.next_direction = Some(direction);
        }
        self.advance_queue_edits();
    }

    /// 菜单编辑与快捷键移动共用串行提交，保留菜单创建时的定位供服务端校验。
    pub(crate) fn apply_queue_edit(&mut self, op: QueueOp) {
        self.cancel_queue_reorder();
        self.queue_edits.queued_operations.push_back(op);
        self.advance_queue_edits();
    }

    /// 用户接管选择或离开浮层后，取消未提交的移动及旧请求的光标跟随。
    pub(super) fn cancel_queue_reorder(&mut self) {
        self.queue_edits.reorder = None;
        if let Some(pending) = &mut self.queue_edits.pending {
            pending.destination = None;
        }
    }

    pub(super) fn complete_queue_edit(&mut self, outcome: &Outcome<QueueEditOutcome>) {
        match outcome {
            Outcome::Applied(QueueEditOutcome::Applied { queue_version }) => {
                if let Some(pending) = &mut self.queue_edits.pending {
                    pending.queue_version = Some(*queue_version);
                }
            }
            Outcome::Applied(QueueEditOutcome::NoOp) => {
                self.queue_edits.pending = None;
                self.cancel_queue_reorder();
            }
            _ => {
                // 失败、过期或执行结果未知时终止这段编辑，不重试旧定位。
                self.queue_edits = QueueEdits::default();
            }
        }
        self.advance_queue_edits();
    }

    /// 回执与快照可先后任意到达；只在 App 已应用提交版本后继续执行。
    pub(super) fn advance_queue_edits(&mut self) {
        if let Some(pending) = &self.queue_edits.pending {
            let Some(committed) = pending.queue_version else {
                return;
            };
            let observed = self.state.models.player.versions.queue;
            if observed < Some(committed) {
                return;
            }
            if observed == Some(committed) {
                if let Some(destination) = pending.destination
                    && let Some(reorder) = &mut self.queue_edits.reorder
                {
                    reorder.anchor.index = destination;
                    reorder.queue_version = observed;
                    self.overlays.follow_queue_reorder(destination);
                    self.queue_cursor_memo = Some(destination);
                }
            } else {
                // 已跳过本次提交版本，旧目标下标不再代表同一条目。
                self.cancel_queue_reorder();
            }
            self.queue_edits.pending = None;
        }

        if let Some(op) = self.queue_edits.queued_operations.pop_front() {
            self.submit_queue_edit(op, None);
            return;
        }
        self.submit_next_queue_reorder();
    }

    fn submit_next_queue_reorder(&mut self) {
        let Some(reorder) = &mut self.queue_edits.reorder else {
            return;
        };
        let player = &self.state.models.player;
        if reorder.queue_version != player.versions.queue {
            self.cancel_queue_reorder();
            return;
        }
        if let Some(direction) = reorder.next_direction.take()
            && let Some(destination) =
                direction.destination(reorder.anchor.index, player.queue.len())
        {
            let op = QueueOp::Move {
                at: reorder.anchor.clone(),
                to: direction.position(),
            };
            self.submit_queue_edit(op, Some(destination));
            return;
        }
        self.queue_edits.reorder = None;
    }

    fn submit_queue_edit(&mut self, op: QueueOp, destination: Option<usize>) {
        self.queue_edits.pending = Some(PendingQueueEdit {
            queue_version: None,
            destination,
        });
        self.client.queue_edit(op);
    }
}

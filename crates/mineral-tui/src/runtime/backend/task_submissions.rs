//! 统一保留尚未发出的后台任务；暂时没有容量时等待下一 tick，业务调用方无需重试。

use std::collections::VecDeque;

use mineral_client::operation::SubmitError;
use mineral_task::{DedupKey, Priority, TaskKind};

/// 后端持有的待提交任务；成功交给 client 后移除，不重发结果未知的请求。
#[derive(Default)]
pub(super) struct TaskSubmissions {
    /// 等待本地发送队列接收的任务，保留同优先级的提交顺序。
    pending: VecDeque<PendingTask>,

    /// 本轮积压的原因，用于合并日志；断连后不再尝试本会话的任务。
    blocked: Option<SubmitError>,
}

/// 一份仍未发出的业务任务。
struct PendingTask {
    /// 任务及其业务参数。
    kind: TaskKind,

    /// 与 daemon scheduler 一致的去重身份。
    key: DedupKey,

    /// 用户需求优先于后台预热。
    priority: Priority,
}

impl TaskSubmissions {
    /// 接收业务提交；同一待提交任务只保留一次，重复的用户请求提升其优先级。
    pub(super) fn enqueue(&mut self, kind: TaskKind, priority: Priority) {
        if self.blocked == Some(SubmitError::Disconnected) {
            return;
        }
        let key = kind.dedup_key();
        if let Some(task) = self.pending.iter_mut().find(|task| task.key == key) {
            task.priority = task.priority.max(priority);
            return;
        }
        self.pending.push_back(PendingTask {
            kind,
            key,
            priority,
        });
    }

    /// 当前关注的任务移到最前；已经发出的任务不受影响，也不会重新提交。
    pub(super) fn prioritize(&mut self, kind: &TaskKind) {
        let key = kind.dedup_key();
        if let Some(index) = self.pending.iter().position(|task| task.key == key)
            && let Some(mut task) = self.pending.remove(index)
        {
            task.priority = Priority::User;
            self.pending.push_front(task);
        }
    }

    /// 待提交数量，供心跳和受阻日志记录。
    pub(super) fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// 已受阻时由每 tick 的统一推进重试，不在同一批业务提交中反复撞容量上限。
    pub(super) fn is_blocked(&self) -> bool {
        self.blocked.is_some()
    }

    /// 尽量发送待提交任务；容量不足保留当前项和余项，断连停止，成功项立即移除。
    ///
    /// # Params:
    ///   - `send`: 非阻塞的 client 提交接口，`Err` 必须表示请求尚未入队。
    pub(super) fn flush(
        &mut self,
        mut send: impl FnMut(TaskKind, Priority) -> Result<(), SubmitError>,
    ) {
        if self.blocked == Some(SubmitError::Disconnected) {
            return;
        }
        while !self.pending.is_empty() {
            let index = self
                .pending
                .iter()
                .position(|task| task.priority == Priority::User)
                .unwrap_or(0);
            let Some(task) = self.pending.remove(index) else {
                break;
            };
            match send(task.kind.clone(), task.priority) {
                Ok(()) => {
                    mineral_log::debug!(target: "task_submit", task = ?task.key, priority = ?task.priority, "后台任务已提交");
                }
                Err(error) => {
                    if self.blocked != Some(error) {
                        mineral_log::warn!(target: "task_submit", task = ?task.key, reason = ?error, pending = self.pending.len() + 1, "后台任务提交受阻，保留未提交任务");
                    }
                    self.blocked = Some(error);
                    self.pending.push_front(task);
                    return;
                }
            }
        }
        if let Some(reason) = self.blocked.take() {
            mineral_log::info!(target: "task_submit", ?reason, pending = 0, "后台任务提交恢复，积压已全部提交");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TaskSubmissions;
    use mineral_channel_core::Page;
    use mineral_client::operation::SubmitError;
    use mineral_model::{AlbumId, ArtistId, PlaylistId, SearchKind, SourceKind};
    use mineral_task::{ChannelFetchKind, Priority, TaskKind};

    /// 混合歌单、专辑、艺人详情和搜索，验证提交层不按业务类型决定重试。
    fn task(index: usize) -> TaskKind {
        let id = index.to_string();
        TaskKind::ChannelFetch(match index % 4 {
            0 => ChannelFetchKind::PlaylistDetail {
                id: PlaylistId::new(SourceKind::NETEASE, id),
                load: mineral_channel_core::PlaylistLoad::Complete,
            },
            1 => ChannelFetchKind::AlbumDetail {
                id: AlbumId::new(SourceKind::NETEASE, id),
            },
            2 => ChannelFetchKind::ArtistDetail {
                id: ArtistId::new(SourceKind::NETEASE, id),
            },
            _ => ChannelFetchKind::Search {
                source: SourceKind::NETEASE,
                kind: SearchKind::Song,
                query: id,
                page: Page::default(),
            },
        })
    }

    /// 超过容量的混合任务逐轮全部发出；未提交项去重，已发出项不因缺少结果而重发。
    #[test]
    fn all_task_kinds_resume_after_capacity_recovers() {
        for error in [SubmitError::QueueFull, SubmitError::InFlightLimit] {
            let mut tasks = TaskSubmissions::default();
            let expected = (0..1500).map(task).collect::<Vec<_>>();
            for kind in &expected {
                tasks.enqueue(kind.clone(), Priority::Background);
                tasks.enqueue(kind.clone(), Priority::Background);
            }
            let mut received = Vec::new();
            for _ in 0..12 {
                let mut capacity = 128;
                tasks.flush(|kind, _| {
                    if capacity == 0 {
                        return Err(error);
                    }
                    capacity -= 1;
                    received.push(kind);
                    Ok(())
                });
                assert_eq!(tasks.pending_count(), expected.len() - received.len());
            }
            assert_eq!(received, expected);
            assert_eq!(tasks.pending_count(), 0);
            assert!(!tasks.is_blocked());
            tasks.flush(|kind, _| {
                received.push(kind);
                Ok(())
            });
            assert_eq!(received, expected);
        }
    }

    /// 受阻期间的新用户需求和当前选中任务优先于后台积压，仍未提交的任务只发一次。
    #[test]
    fn user_and_selected_tasks_precede_background_backlog() {
        let mut tasks = TaskSubmissions::default();
        tasks.enqueue(task(0), Priority::Background);
        tasks.flush(|_, _| Err(SubmitError::QueueFull));
        tasks.enqueue(task(1), Priority::Background);
        tasks.enqueue(task(2), Priority::User);
        tasks.prioritize(&task(1));
        let mut received = Vec::new();
        tasks.flush(|kind, priority| {
            received.push((kind, priority));
            Ok(())
        });
        assert_eq!(
            received,
            vec![
                (task(1), Priority::User),
                (task(2), Priority::User),
                (task(0), Priority::Background)
            ]
        );
    }

    /// 断连后停止；已经发出但没有结果的任务不会重新进入待提交队列。
    #[test]
    fn disconnect_does_not_replay_unknown_requests() {
        let mut tasks = TaskSubmissions::default();
        tasks.enqueue(task(0), Priority::User);
        tasks.enqueue(task(1), Priority::Background);
        tasks.flush(|kind, _| {
            if kind == task(0) {
                Ok(())
            } else {
                Err(SubmitError::Disconnected)
            }
        });
        assert_eq!(tasks.pending_count(), 1);
        let mut called = false;
        tasks.flush(|_, _| {
            called = true;
            Ok(())
        });
        assert!(!called);
    }
}

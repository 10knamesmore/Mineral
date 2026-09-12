//! 喜欢状态操作与本地播放统计查询。

use std::sync::Arc;

use mineral_model::{Song, SongId};
use mineral_protocol::{FailureKind, OperationResult};
use mineral_task::TaskEvent;

use super::Client;
use crate::operation::{Outcome, Pending, SubmitError};

impl Client {
    /// 切换喜欢状态。
    ///
    /// # Params:
    ///   - `song`: 目标歌曲
    ///
    /// # Errors
    /// 本地未提交。
    pub fn toggle_love(&self, song: Song) -> Result<Pending<bool>, SubmitError> {
        self.submit(
            mineral_protocol::Request::ToggleLove(Box::new(song)),
            |result, request_name| match result {
                OperationResult::Query(response) => match *response {
                    mineral_protocol::Response::LoveToggled(loved) => Outcome::Applied(loved),
                    other => Outcome::Failed {
                        kind: FailureKind::Internal,
                        detail: format!("{request_name} 收到意外应答 {other:?}"),
                    },
                },
                OperationResult::Failed(failure) => Outcome::Failed {
                    kind: failure.kind,
                    detail: failure.detail,
                },
                _ => Outcome::Failed {
                    kind: FailureKind::Internal,
                    detail: format!("{request_name} 缺少结果载荷"),
                },
            },
        )
    }

    /// 请求一首歌的本地播放统计;结果以 [`TaskEvent::LocalPlayCountFetched`] 进事件流。
    ///
    /// # Params:
    ///   - `id`: 目标歌曲
    pub fn request_song_stats(&self, id: SongId) {
        let query_id = id.clone();
        let event_id = id;
        let pending = match self.submit(
            mineral_protocol::Request::QuerySongStats(query_id),
            move |result, _request_name| {
                let count = match result {
                    OperationResult::Query(response) => match *response {
                        mineral_protocol::Response::SongStats(stats) => stats.map(|s| s.play_count),
                        _ => None,
                    },
                    _ => None,
                };
                Outcome::Applied(count)
            },
        ) {
            Ok(pending) => pending,
            Err(error) => {
                mineral_log::warn!(target: "ipc", error = %error, "播放统计查询未提交");
                self.mirror()
                    .push_event(mineral_protocol::Event::Task(Box::new(
                        TaskEvent::LocalPlayCountFetched {
                            song_id: event_id,
                            count: None,
                        },
                    )));
                return;
            }
        };
        let mirror = Arc::clone(self.mirror());
        tokio::spawn(async move {
            let count = pending.outcome().await.into_success().flatten();
            mirror.push_event(mineral_protocol::Event::Task(Box::new(
                TaskEvent::LocalPlayCountFetched {
                    song_id: event_id,
                    count,
                },
            )));
        });
    }
}

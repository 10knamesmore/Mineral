//! 保存与管理用户歌单。
use super::Client;
use crate::operation::{Pending, SubmitError, decode_applied, decode_query};
use mineral_model::PlaylistId;
use mineral_protocol::{PlaylistOp, Request, Response};

impl Client {
    /// 保存 daemon 处理请求时的完整队列，返回新歌单身份。
    ///
    /// # Errors
    /// 请求未进入本地发送队列。
    pub fn save_queue_as_playlist(&self, name: String) -> Result<Pending<PlaylistId>, SubmitError> {
        self.submit(
            Request::Playlist {
                op: PlaylistOp::SaveQueue { name },
            },
            |result, request| {
                decode_query(result, request, |response| match response {
                    Response::PlaylistSaved { id } => Some(id),
                    _ => None,
                })
            },
        )
    }

    /// 按身份修改自建歌单名。
    ///
    /// # Errors
    /// 请求未进入本地发送队列。
    pub fn rename_playlist(
        &self,
        id: PlaylistId,
        name: String,
    ) -> Result<Pending<()>, SubmitError> {
        self.submit(
            Request::Playlist {
                op: PlaylistOp::Rename { id, name },
            },
            decode_applied,
        )
    }

    /// 删除自建歌单，保留歌曲和队列。
    ///
    /// # Errors
    /// 请求未进入本地发送队列。
    pub fn delete_playlist(&self, id: PlaylistId) -> Result<Pending<()>, SubmitError> {
        self.submit(
            Request::Playlist {
                op: PlaylistOp::Delete { id },
            },
            decode_applied,
        )
    }
}

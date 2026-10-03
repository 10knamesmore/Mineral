//! 自建歌单的队列取值、管理资格与操作回执。

use crate::player::PlayerCore;
use mineral_model::{PlaylistId, SourceKind};
use mineral_protocol::{FailureKind, PlaylistOp};
use mineral_stats::{PlaylistOpKind, PlaylistRef};

/// 歌单请求的业务失败。
#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    /// 名称为空或包含控制字符。
    #[error("playlist name must contain visible text and no control characters")]
    InvalidName,

    /// 没有可保存的队列曲目。
    #[error("queue is empty")]
    EmptyQueue,

    /// 来源或内置身份不允许管理。
    #[error("playlist {0} cannot be managed")]
    Protected(PlaylistId),

    /// 持久读写失败。
    #[error(transparent)]
    Store(#[from] crate::persistence::Error),
}

impl Error {
    /// 将领域失败映射为客户端可消费的类别。
    pub(crate) fn kind(&self) -> FailureKind {
        match self {
            Self::InvalidName | Self::EmptyQueue => FailureKind::Invalid,
            Self::Protected(_) | Self::Store(crate::persistence::Error::Disabled) => {
                FailureKind::Unavailable
            }
            Self::Store(crate::persistence::Error::PlaylistNotFound { .. }) => {
                FailureKind::NotFound
            }
            Self::Store(_) => FailureKind::Internal,
        }
    }
}

impl PlayerCore {
    /// 请求处理时取队列，提交持久写入后刷新客户端的歌单库。
    pub(crate) async fn playlist_operation(
        &self,
        op: PlaylistOp,
    ) -> Result<Option<PlaylistId>, Error> {
        let (kind, reference) = match &op {
            PlaylistOp::SaveQueue { name } => (
                PlaylistOpKind::Create,
                PlaylistRef::Creating {
                    source: SourceKind::MINERAL,
                    name: name.clone(),
                },
            ),
            PlaylistOp::Rename { id, .. } => {
                (PlaylistOpKind::Rename, PlaylistRef::Existing(id.clone()))
            }
            PlaylistOp::Delete { id } => {
                (PlaylistOpKind::Delete, PlaylistRef::Existing(id.clone()))
            }
        };
        // 在任何 await 之前取得快照，后续队列修改不影响本次保存。
        let songs = matches!(&op, PlaylistOp::SaveQueue { .. })
            .then(|| self.inner.state.lock().queue.clone());
        let result = match op {
            PlaylistOp::SaveQueue { name } => {
                let name = checked_name(&name)?;
                let songs = songs.as_deref().ok_or(Error::EmptyQueue)?;
                if songs.is_empty() {
                    return Err(Error::EmptyQueue);
                }
                self.persist()
                    .create_user_playlist(name, songs)
                    .await
                    .map(Some)
                    .map_err(Into::into)
            }
            PlaylistOp::Rename { id, name } => {
                check_managed(&id)?;
                self.persist()
                    .rename_user_playlist(&id, checked_name(&name)?)
                    .await?;
                Ok(None)
            }
            PlaylistOp::Delete { id } => {
                check_managed(&id)?;
                self.persist().delete_user_playlist(&id).await?;
                Ok(None)
            }
        };
        let count = songs.as_ref().map_or(0, Vec::len);
        let reference = match &result {
            Ok(Some(id)) => PlaylistRef::Existing(id.clone()),
            _ => reference,
        };
        mineral_log::info!(target: "playlists", operation = ?kind, playlist = ?reference, songs = count, success = result.is_ok(), "playlist operation finished");
        self.inner.stats.event(mineral_stats::StatsEvent::Behavior {
            actor: mineral_stats::Actor::User,
            event: mineral_stats::BehaviorEvent::PlaylistOp {
                op: kind,
                playlist_ref: reference,
                song: None,
                song_count: i64::try_from(count).unwrap_or(i64::MAX),
                outcome: if result.is_ok() {
                    mineral_stats::OpOutcome::Ok
                } else {
                    mineral_stats::OpOutcome::Failed
                },
                error_kind: result
                    .as_ref()
                    .err()
                    .map(|_| mineral_stats::PlaylistError::Other),
            },
        });
        if result.is_ok() {
            self.submit_my_playlists(SourceKind::MINERAL);
        }
        result
    }
}

/// 接受非空的单行名称，忽略首尾空白。
fn checked_name(name: &str) -> Result<&str, Error> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(Error::InvalidName);
    }
    Ok(name)
}

/// 内置歌单和其他来源都不能经本地管理操作修改。
fn check_managed(id: &PlaylistId) -> Result<(), Error> {
    if id.namespace() != SourceKind::MINERAL || id.value() == "favorites" {
        return Err(Error::Protected(id.clone()));
    }
    Ok(())
}

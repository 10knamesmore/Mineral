//! 请求分派:同步有序操作内联提交,慢操作并发执行,结果结构化返回。
//!
//! 顺序语义:同一 client 的播放 / 队列操作在 read loop 内**按到达次序 await**,
//! 不依赖 spawn 调度碰巧有序;查询与脚本 / 数据库慢操作并发执行,结果经 id 配对。
//! 跨 client 以 daemon 的接受次序仲裁(各自 read loop 串行)。

use mineral_protocol::{
    CopyTextFailure, FailureKind, OperationFailure, OperationResult, Request, Response,
};

use crate::client::ClientHandle;

/// 需要并发执行的慢请求(自带业务语义)。
pub(crate) enum AsyncRequest {
    /// Output enumeration may call the system audio service.
    AudioOutputs,

    /// 脚本具名动作。
    InvokeAction {
        /// 动作名。
        name: String,

        /// 按键上下文。
        ctx: Option<mineral_protocol::KeyContext>,

        /// 位置实参。
        args: Vec<String>,
    },

    /// 复制模板渲染。
    RenderCopyTemplate {
        /// 模板下标。
        index: usize,

        /// 模板实体。
        ctx: mineral_protocol::CopyTemplateCtx,
    },

    /// 读 per-song 持久值。
    StoreGet {
        /// 目标歌。
        song: mineral_model::SongId,

        /// 开放键。
        key: String,
    },

    /// 写 per-song 持久值。
    StoreSet {
        /// 目标歌。
        song: mineral_model::SongId,

        /// 开放键。
        key: String,

        /// 标量值。
        value: mineral_protocol::StoreValue,
    },

    /// 喜欢切换。
    ToggleLove(Box<mineral_model::Song>),

    /// 本地播放统计查询。
    QuerySongStats(mineral_model::SongId),

    /// 脚本绑定表查询(脚本线程交互,可能阻塞)。
    ScriptBinds,
}

/// 把请求归入并发慢路径;返回 `None` 表示走同步有序路径。
///
/// # Params:
///   - `request`: 业务请求
pub(crate) fn async_request(request: &Request) -> Option<AsyncRequest> {
    match request {
        Request::AudioOutputs => Some(AsyncRequest::AudioOutputs),
        Request::InvokeAction { name, ctx, args } => Some(AsyncRequest::InvokeAction {
            name: name.clone(),
            ctx: ctx.clone(),
            args: args.clone(),
        }),
        Request::RenderCopyTemplate { index, ctx } => Some(AsyncRequest::RenderCopyTemplate {
            index: *index,
            ctx: ctx.clone(),
        }),
        Request::StoreGet { song, key } => Some(AsyncRequest::StoreGet {
            song: song.clone(),
            key: key.clone(),
        }),
        Request::StoreSet { song, key, value } => Some(AsyncRequest::StoreSet {
            song: song.clone(),
            key: key.clone(),
            value: value.clone(),
        }),
        Request::ToggleLove(song) => Some(AsyncRequest::ToggleLove(song.clone())),
        Request::QuerySongStats(id) => Some(AsyncRequest::QuerySongStats(id.clone())),
        Request::ScriptBinds => Some(AsyncRequest::ScriptBinds),
        _ => None,
    }
}

/// 执行一条同步请求(read loop 内联 await,保持到达次序)。
///
/// # Params:
///   - `client`: 业务句柄
///   - `request`: 业务请求
pub(crate) fn execute_sync(client: &ClientHandle, request: Request) -> OperationResult {
    match request {
        Request::Pause => {
            client.pause();
            OperationResult::Applied
        }
        Request::Resume => {
            client.resume();
            OperationResult::Applied
        }
        Request::Stop => {
            client.stop();
            OperationResult::Applied
        }
        Request::Seek(ms) => {
            client.seek(ms);
            OperationResult::Applied
        }
        Request::SetVolume(pct) => {
            client.set_volume(pct);
            OperationResult::Applied
        }
        Request::PlaySong(song) => {
            client.play_song(&song);
            OperationResult::Applied
        }
        Request::PlayQueue {
            songs,
            target,
            context,
        } => {
            // 保留结构化业务错误,供 client 按类别处理。
            query(Response::PlayQueue(
                client.play_queue(songs, target, context),
            ))
        }
        Request::QueueInsertNext { songs, context } => {
            client.queue_insert_next(songs, context);
            OperationResult::Applied
        }
        Request::QueueAppend { songs, context } => {
            client.queue_append(songs, context);
            OperationResult::Applied
        }
        Request::CyclePlayMode => {
            client.cycle_play_mode();
            OperationResult::Applied
        }
        Request::SetPlayMode(mode) => {
            client.set_play_mode(mode);
            OperationResult::Applied
        }
        Request::PrevOrRestart => {
            client.prev_or_restart();
            OperationResult::Applied
        }
        Request::NextSong => {
            client.next_song();
            OperationResult::Applied
        }
        Request::SubmitTask(kind, priority) => {
            client.submit_task(kind, priority);
            OperationResult::Accepted
        }
        Request::Download(target) => {
            client.download(target);
            OperationResult::Accepted
        }
        Request::StopDownload(id) => match client.stop_download(&id) {
            Ok(()) => OperationResult::Applied,
            Err(error) => failure(&error, FailureKind::NotFound),
        },
        Request::ChannelCaps => query(Response::ChannelCaps(client.channel_caps())),
        Request::DaemonInfo => query(Response::DaemonInfo {
            pid: std::process::id(),
        }),
        Request::TerminalState {
            rows,
            cols,
            fullscreen,
            focused,
        } => {
            client.report_terminal_state(rows, cols, fullscreen, focused);
            OperationResult::Applied
        }
        Request::Shutdown => {
            mineral_log::info!(target: "ipc", "shutdown requested via IPC");
            OperationResult::Applied
        }
        other => {
            mineral_log::error!(target: "ipc", request = ?other, "request reached wrong dispatch lane");
            OperationResult::Failed(OperationFailure {
                kind: FailureKind::Internal,
                detail: "操作失败".to_owned(),
            })
        }
    }
}

/// 执行一条并发慢请求。
///
/// # Params:
///   - `client`: 业务句柄
///   - `request`: 慢请求
pub(crate) async fn execute_async(client: &ClientHandle, request: AsyncRequest) -> OperationResult {
    match request {
        AsyncRequest::AudioOutputs => match client.audio_outputs().await {
            Ok(devices) => query(Response::AudioOutputs(devices)),
            Err(error) => failure(&error, audio_failure_kind(&error)),
        },
        AsyncRequest::InvokeAction { name, ctx, args } => {
            match client.invoke_action_async(&name, ctx, args).await {
                Ok(()) => OperationResult::Applied,
                Err(error) => failure(&error, action_failure_kind(&error)),
            }
        }
        AsyncRequest::RenderCopyTemplate { index, ctx } => {
            let text = client.render_copy_template_async(index, ctx).await.map_err(|error| {
                let detail = mineral_log::chain(&error);
                mineral_log::warn!(target: "ipc", error = detail.as_str(), "copy template failed");
                match error {
                    crate::notify::ScriptError::Disabled => CopyTextFailure::ScriptDisabled,
                    crate::notify::ScriptError::ThreadExited
                    | crate::notify::ScriptError::Callback(mineral_script::Error::Unavailable) => {
                        CopyTextFailure::ScriptThreadExited
                    }
                    crate::notify::ScriptError::ActionNotFound(_)
                    | crate::notify::ScriptError::Callback(_) => CopyTextFailure::CallbackFailed { detail },
                }
            });
            query(Response::CopyText(text))
        }
        AsyncRequest::StoreGet { song, key } => match client.store_get_async(&song, &key).await {
            Ok(value) => query(Response::StoreValue(value)),
            Err(error) => failure(&error, store_failure_kind(&error)),
        },
        AsyncRequest::StoreSet { song, key, value } => {
            match client.store_set_async(&song, &key, &value).await {
                Ok(()) => OperationResult::Applied,
                Err(error) => failure(&error, store_failure_kind(&error)),
            }
        }
        AsyncRequest::ToggleLove(song) => match client.toggle_love_async(&song).await {
            Ok(loved) => query(Response::LoveToggled(loved)),
            Err(error) => failure(&error, FailureKind::Internal),
        },
        AsyncRequest::QuerySongStats(id) => match client.query_song_stats_async(&id).await {
            Ok(stats) => query(Response::SongStats(stats)),
            Err(error) => failure(&error, FailureKind::Internal),
        },
        AsyncRequest::ScriptBinds => {
            query(Response::ScriptBinds(client.script_binds_async().await))
        }
    }
}

/// 队列编辑(有序,但可能跨线程跑脚本变换,需在 read loop 内联 await)。
///
/// # Params:
///   - `client`: 业务句柄
///   - `op`: 编辑操作
pub(crate) async fn execute_queue_edit(
    client: &ClientHandle,
    op: mineral_protocol::QueueOp,
) -> OperationResult {
    query(Response::QueueEdited(client.queue_edit_async(op).await))
}

/// 业务失败 → 结构化结果。完整错误链只记日志,wire 仅携带安全类别。
///
/// # Params:
///   - `error`: 操作的原始错误
///   - `kind`: 从操作或错误变体决定的稳定分类
pub(crate) fn failure(
    error: &(dyn std::error::Error + 'static),
    kind: FailureKind,
) -> OperationResult {
    mineral_log::warn!(target: "ipc", error = mineral_log::chain(error), failure_kind = ?kind, "RPC request failed");
    OperationResult::Failed(OperationFailure {
        kind,
        detail: match kind {
            FailureKind::Invalid => "请求参数不合法",
            FailureKind::NotFound => "目标不存在",
            FailureKind::Unavailable => "服务暂不可用",
            FailureKind::Conflict => "当前状态不允许该操作",
            FailureKind::Internal => "操作失败",
        }
        .to_owned(),
    })
}

/// Classifies a script action without inspecting its display text.
fn action_failure_kind(error: &crate::notify::ScriptError) -> FailureKind {
    match error {
        crate::notify::ScriptError::Disabled
        | crate::notify::ScriptError::ThreadExited
        | crate::notify::ScriptError::Callback(mineral_script::Error::Unavailable) => {
            FailureKind::Unavailable
        }
        crate::notify::ScriptError::ActionNotFound(_)
        | crate::notify::ScriptError::Callback(mineral_script::Error::MissingFunction { .. }) => {
            FailureKind::NotFound
        }
        crate::notify::ScriptError::Callback(
            mineral_script::Error::InvalidSongId { .. }
            | mineral_script::Error::InvalidSongEntry { .. },
        ) => FailureKind::Invalid,
        crate::notify::ScriptError::Callback(
            mineral_script::Error::Lua { .. }
            | mineral_script::Error::Child { .. }
            | mineral_script::Error::Thread(_),
        ) => FailureKind::Internal,
    }
}

/// Classifies persistent KV validation separately from storage failures.
fn store_failure_kind(error: &mineral_persist::Error) -> FailureKind {
    match error {
        mineral_persist::Error::ReservedKey { .. } | mineral_persist::Error::NotInteger { .. } => {
            FailureKind::Invalid
        }
        _ => FailureKind::Internal,
    }
}

/// Classifies audio device validation separately from output availability.
pub(crate) fn audio_failure_kind(error: &mineral_audio::Error) -> FailureKind {
    match error {
        mineral_audio::Error::InvalidDeviceId { .. } => FailureKind::Invalid,
        mineral_audio::Error::DeviceUnavailable { .. } => FailureKind::NotFound,
        mineral_audio::Error::OutputUnavailable
        | mineral_audio::Error::OutputDisabled
        | mineral_audio::Error::NoDefaultDevice => FailureKind::Unavailable,
        _ => FailureKind::Internal,
    }
}

/// 查询类应答包装。
///
/// # Params:
///   - `response`: 查询载荷
pub(crate) fn query(response: Response) -> OperationResult {
    OperationResult::Query(Box::new(response))
}

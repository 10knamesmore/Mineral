//! 请求分派:同步有序操作内联提交,慢操作并发执行,结果结构化返回。
//!
//! 顺序语义:同一 client 的播放 / 队列操作在 read loop 内**按到达次序 await**,
//! 不依赖 spawn 调度碰巧有序;查询与脚本 / 数据库慢操作并发执行,结果经 id 配对。
//! 跨 client 以 daemon 的接受次序仲裁(各自 read loop 串行)。

use mineral_protocol::{FailureKind, OperationFailure, OperationResult, Request, Response};

use crate::client::ClientHandle;

/// 需要并发执行的慢请求(自带业务语义)。
pub(crate) enum AsyncRequest {
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
        Request::QueueInsertNext { song, context } => {
            client.queue_insert_next(*song, context);
            OperationResult::Applied
        }
        Request::QueueAppend { song, context } => {
            client.queue_append(*song, context);
            OperationResult::Applied
        }
        Request::CyclePlayMode => {
            client.cycle_play_mode();
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
            Err(error) => failure(&error),
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
        other => OperationResult::Failed(OperationFailure {
            kind: FailureKind::Internal,
            detail: format!("请求未在同步分派中处理: {other:?}"),
        }),
    }
}

/// 执行一条并发慢请求。
///
/// # Params:
///   - `client`: 业务句柄
///   - `request`: 慢请求
pub(crate) async fn execute_async(client: &ClientHandle, request: AsyncRequest) -> OperationResult {
    match request {
        AsyncRequest::InvokeAction { name, ctx, args } => {
            match client.invoke_action_async(&name, ctx, args).await {
                Ok(()) => OperationResult::Applied,
                Err(error) => failure(&error),
            }
        }
        AsyncRequest::RenderCopyTemplate { index, ctx } => {
            let text = client.render_copy_template_async(index, ctx).await;
            query(Response::CopyText(text))
        }
        AsyncRequest::StoreGet { song, key } => match client.store_get_async(&song, &key).await {
            Ok(value) => query(Response::StoreValue(value)),
            Err(error) => failure(&error),
        },
        AsyncRequest::StoreSet { song, key, value } => {
            match client.store_set_async(&song, &key, &value).await {
                Ok(()) => OperationResult::Applied,
                Err(error) => failure(&error),
            }
        }
        AsyncRequest::ToggleLove(song) => match client.toggle_love_async(&song).await {
            Ok(loved) => query(Response::LoveToggled(loved)),
            Err(error) => failure(&error),
        },
        AsyncRequest::QuerySongStats(id) => match client.query_song_stats_async(&id).await {
            Ok(stats) => query(Response::SongStats(stats)),
            Err(error) => failure(&error),
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

/// 业务失败 → 结构化结果。
///
/// # Params:
///   - `error`: 错误链(经 [`mineral_log::chain`] 展开)
pub(crate) fn failure(error: &color_eyre::Report) -> OperationResult {
    let detail = mineral_log::chain(error);
    OperationResult::Failed(OperationFailure {
        kind: classify_detail(&detail),
        detail,
    })
}

/// 按错误文本中的关键词推断失败类别;未命中时归为 `Internal`。
///
/// # Params:
///   - `detail`: 人读错误文本
pub(crate) fn classify_detail(detail: &str) -> FailureKind {
    if detail.contains("未启用") || detail.contains("unavailable") || detail.contains("未登录")
    {
        FailureKind::Unavailable
    } else if detail.contains("未注册") || detail.contains("unknown") {
        FailureKind::NotFound
    } else if detail.contains("越界")
        || detail.contains("不能为空")
        || detail.contains("超过上限")
        || detail.contains("不支持")
    {
        FailureKind::Invalid
    } else {
        FailureKind::Internal
    }
}

/// 查询类应答包装。
///
/// # Params:
///   - `response`: 查询载荷
pub(crate) fn query(response: Response) -> OperationResult {
    OperationResult::Query(Box::new(response))
}

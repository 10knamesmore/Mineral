//! 待发命令自动合批、在途登记与会话写入。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use mineral_protocol::{MessageBatch, SessionMessage, SessionRequest, SubscriptionId, WireSink};
use tokio::sync::mpsc;

use super::SessionShared;
use super::lifecycle::Command;
use crate::connection::ClientConfig;

/// writer:吸干当前可用命令 → 组装一批消息 → 一次 sink send。不等待凑满、不定时。
pub(super) async fn writer_loop(
    mut sink: Box<dyn WireSink>,
    mut rx: mpsc::Receiver<Command>,
    mut resync_rx: mpsc::UnboundedReceiver<SubscriptionId>,
    shared: Arc<SessionShared>,
    config: ClientConfig,
) {
    let mut buf = Vec::with_capacity(config.max_batch);
    loop {
        let first = tokio::select! {
            biased;
            () = shared.cancel.cancelled() => break,
            maybe = rx.recv() => match maybe {
                Some(command) => command,
                None => break,
            },
            Some(id) = resync_rx.recv() => Command::Resync(id),
        };
        buf.clear();
        buf.push(first);
        while buf.len() < config.max_batch {
            match rx.try_recv() {
                Ok(command) => buf.push(command),
                Err(_) => break,
            }
        }
        while buf.len() < config.max_batch {
            match resync_rx.try_recv() {
                Ok(id) => buf.push(Command::Resync(id)),
                Err(_) => break,
            }
        }
        let mut close_after = false;
        let batch = build_batch(&mut buf, &shared, &mut close_after);
        let message_count = batch.messages.len();
        if message_count > 0
            && let Err(error) = sink.send(batch).await
        {
            mineral_log::warn!(target: "ipc", error = mineral_log::chain(&error), "会话写失败,收束会话");
            shared.disconnect();
            break;
        }
        shared.batches.fetch_add(1, Ordering::Relaxed);
        shared.messages.fetch_add(
            u64::try_from(message_count).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        if close_after {
            break;
        }
    }
    let _ = sink.close().await;
    shared.disconnect();
}

/// 把一批命令转成会话消息;请求先登记在途表(先登记后发送)。
fn build_batch(
    commands: &mut Vec<Command>,
    shared: &SessionShared,
    close_after: &mut bool,
) -> MessageBatch {
    let mut messages = Vec::new();
    let mut requests: Vec<SessionRequest> = Vec::new();
    for command in commands.drain(..) {
        match command {
            Command::Request { id, request, reply } => {
                shared.inflight.lock().insert(id, reply);
                requests.push(SessionRequest { id, request });
            }
            Command::Subscribe(request) => {
                flush_requests(&mut messages, &mut requests);
                messages.push(SessionMessage::Subscribe(request));
            }
            Command::Unsubscribe(id) => {
                flush_requests(&mut messages, &mut requests);
                messages.push(SessionMessage::Unsubscribe(id));
            }
            Command::Resync(id) => {
                flush_requests(&mut messages, &mut requests);
                messages.push(SessionMessage::Resync(id));
            }
            Command::Close(reason) => {
                flush_requests(&mut messages, &mut requests);
                messages.push(SessionMessage::Close(reason));
                *close_after = true;
            }
        }
    }
    flush_requests(&mut messages, &mut requests);
    let request_count = messages
        .iter()
        .map(|message| match message {
            SessionMessage::Requests(batch) => batch.len(),
            _ => 0,
        })
        .sum::<usize>();
    shared.requests.fetch_add(
        u64::try_from(request_count).unwrap_or(u64::MAX),
        Ordering::Relaxed,
    );
    MessageBatch { messages }
}

/// 把累积的请求合成一条 `Requests` 消息(连续请求合并,保持相对次序)。
fn flush_requests(messages: &mut Vec<SessionMessage>, requests: &mut Vec<SessionRequest>) {
    if requests.is_empty() {
        return;
    }
    messages.push(SessionMessage::Requests(std::mem::take(requests)));
}

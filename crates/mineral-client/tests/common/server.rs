//! 模拟服务端的握手应答和请求批次读取工具。

use color_eyre::eyre::eyre;
use mineral_protocol::{MessageBatch, RequestId, ServerHello, SessionMessage, Wire};

/// server 侧完成握手:读 Hello、回 Welcome。
///
/// # Params:
///   - `server`: server 侧承载
pub(crate) async fn accept_handshake(server: &mut Box<dyn Wire>) -> color_eyre::Result<()> {
    let batch = server
        .recv()
        .await?
        .ok_or_else(|| eyre!("握手期间对端关闭"))?;
    assert!(
        matches!(batch.messages.first(), Some(SessionMessage::Hello(_))),
        "首帧应为 Hello,实际 {batch:?}"
    );
    server
        .send(MessageBatch::one(SessionMessage::Welcome(
            ServerHello::accept(),
        )))
        .await?;
    Ok(())
}

/// 从一批消息里抽出全部请求 id(按批内顺序)。
///
/// # Params:
///   - `batch`: 收到的会话批
pub(crate) fn request_ids(batch: &MessageBatch) -> Vec<RequestId> {
    batch
        .messages
        .iter()
        .flat_map(|message| match message {
            SessionMessage::Requests(requests) => requests
                .iter()
                .map(|request| request.id)
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect()
}

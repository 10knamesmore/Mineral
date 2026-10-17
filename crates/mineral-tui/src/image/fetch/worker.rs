//! 分发图片请求：显示结果进入完成队列，源图结果直接回复请求者。

use std::sync::Arc;

use crate::config::CoverConfig;
use isahc::HttpClient;
use mineral_persist::CacheIndex;

use super::decode::{fetch_and_decode, fetch_original};
use super::preview::fetch_preview;
use super::types::{CoverCompletion, CoverReady, CoverRequest, CoverRequestKind, DecodeRequest};

/// 执行图片请求；源图通过请求内的通道回复，其余请求返回显示完成事件。
///
/// # Params:
///   - `request`: preview、显示解码或源图解码请求
///   - `client`: Remote 图片 HTTP 客户端
///   - `cache`: 可用的磁盘缓存
///   - `cfg`: 解码与取色配置
///
/// # Return:
///   显示请求返回成功产物或失败事件；源图请求已直接回复，返回 `None`
pub(super) async fn complete_request(
    request: CoverRequest,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
    cfg: &Arc<CoverConfig>,
) -> Option<CoverCompletion> {
    let completion = match request {
        CoverRequest::Preview(request) => {
            let url = request.url.clone();
            match fetch_preview(request, client, cache, cfg).await {
                Some((preview, None)) => CoverCompletion::Preview(preview),
                Some((preview, Some(full))) => CoverCompletion::PreviewAndDecoded { preview, full },
                None => CoverCompletion::Failed {
                    url,
                    kind: CoverRequestKind::Preview,
                },
            }
        }
        CoverRequest::Decode(DecodeRequest {
            source,
            url,
            pixels,
        }) => {
            if let Some(decoded) = fetch_and_decode(source, &url, client, cache, cfg, pixels).await
            {
                CoverCompletion::Decoded(CoverReady {
                    url,
                    image: Arc::new(decoded.image),
                    fingerprint: decoded.fingerprint,
                    palette: decoded.palette,
                })
            } else {
                CoverCompletion::Failed {
                    url,
                    kind: CoverRequestKind::Decode,
                }
            }
        }
        CoverRequest::Original { source, url, reply } => {
            let result = fetch_original(source, &url, client, cache).await;
            let _ = reply.send(result);
            return None;
        }
    };
    Some(completion)
}

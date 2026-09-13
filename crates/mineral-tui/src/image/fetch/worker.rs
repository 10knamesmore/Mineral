//! 请求分发:把一条 preview / decode 请求派给对应管线,回传必达 completion。

use std::sync::Arc;

use isahc::HttpClient;
use mineral_config::CoverConfig;
use mineral_persist::CacheIndex;

use super::decode::fetch_and_decode;
use super::preview::fetch_preview;
use super::types::{CoverCompletion, CoverReady, CoverRequest, CoverRequestKind, DecodeRequest};

/// 执行一个结构化图片请求并生成必达 completion。
///
/// # Params:
///   - `request`: preview 或 decode 请求
///   - `client`: Remote 图片 HTTP 客户端
///   - `cache`: 可用的磁盘缓存
///   - `cfg`: 解码与取色配置
///
/// # Return:
///   成功产物或带请求类型的失败 completion
pub(super) async fn complete_request(
    request: CoverRequest,
    client: &HttpClient,
    cache: Option<&Arc<CacheIndex>>,
    cfg: &Arc<CoverConfig>,
) -> CoverCompletion {
    match request {
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
    }
}

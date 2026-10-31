//! 封面 worker 池的启动、请求投递与完成结果收集。

use std::sync::Arc;
use std::time::Duration;

use crate::config::{CoverConfig, CoverDecodePixelsConfig};
use crate::persistence::TuiStore;
use image::RgbaImage;
use isahc::HttpClient;
use isahc::config::Configurable;
use mineral_model::{MediaUrl, SourceKind};
use mineral_persist::CacheIndex;
use parking_lot::Mutex;
use tokio::sync::{mpsc, oneshot};

use crate::image::key::PreviewTarget;

use super::types::{
    CoverCompletion, CoverRequest, DecodeRequest, OriginalImageError, PreviewRequest, ReadyBuf,
};
use super::worker::complete_request;

/// 共用下载、解码 worker 与源文件缓存。显示结果通过 `drain_ready` 收集，
/// 源图请求单独回复；禁用态关闭请求队列。
pub(crate) struct CoverFetcher {
    /// 待执行的请求。
    req_tx: mpsc::UnboundedSender<CoverRequest>,

    /// worker 完成后塞结果的 buffer;TUI tick `drain_ready()` 一次拿走。
    ready: ReadyBuf,
}

impl CoverFetcher {
    /// 起 worker 池(数量 = `cfg.download_workers`)。caller 必须在 tokio runtime 里
    /// (`mineral_tui::run` 是 async fn,自然满足);失败通常意味着 isahc 客户端建不起来
    /// (系统证书 / TLS 问题等)。
    ///
    /// # Params:
    ///   - `cfg`: 封面段配置(timeout / 并发 / kmeans)
    ///   - `cover_capacity`: 封面磁盘缓存容量上限(字节,配置 `cover.cache.disk`)
    ///   - `store`: 共享的 `tui.db` 句柄(与 UI 偏好共用连接池;`None` = 降级不缓存)
    pub(crate) async fn spawn(
        cfg: CoverConfig,
        cover_capacity: u64,
        store: Option<Arc<TuiStore>>,
    ) -> Result<Self, isahc::Error> {
        let (tx, rx) = mpsc::unbounded_channel::<CoverRequest>();
        let client = HttpClient::builder()
            .timeout(Duration::from_secs(*cfg.http_timeout_secs()))
            .build()?;
        // 磁盘缓存是优化项:store 不可用 / 目录解析失败不致命,降级成直连网络不缓存。
        let cache = Self::open_cache(store, cover_capacity).await;
        let ready = Arc::new(Mutex::new(Vec::<CoverCompletion>::new()));
        let rx = Arc::new(tokio::sync::Mutex::new(rx));
        let cfg = Arc::new(cfg);
        for _ in 0..(*cfg.download_workers()).max(1) {
            let rx = Arc::clone(&rx);
            let ready = Arc::clone(&ready);
            let client = client.clone();
            let cache = cache.clone();
            let cfg = Arc::clone(&cfg);
            tokio::spawn(async move {
                loop {
                    let request = {
                        let mut g = rx.lock().await;
                        match g.recv().await {
                            Some(item) => item,
                            None => return, // 队列关了
                        }
                    };
                    if let Some(completion) =
                        complete_request(request, &client, cache.as_ref(), &cfg).await
                    {
                        ready.lock().push(completion);
                    }
                }
            });
        }
        Ok(Self { req_tx: tx, ready })
    }

    /// 打开封面磁盘缓存(`cover_cache` 表落共享的 `tui.db`,文件落 `cover_cache_dir`)。
    /// store 不可用 / 目录解析 / open 失败时 warn + 返回 `None`(降级成不缓存),
    /// 不让 fetcher 起步失败。
    ///
    /// # Params:
    ///   - `store`: 共享的 `tui.db` 句柄(`None` = 上游已降级)
    ///   - `capacity`: 缓存容量上限(字节,配置 `cover.cache.disk`)
    ///
    /// # Return:
    ///   就绪的缓存句柄;不可用时 `None`。
    async fn open_cache(store: Option<Arc<TuiStore>>, capacity: u64) -> Option<Arc<CacheIndex>> {
        let store = store?;
        let dir = match mineral_paths::cover_cache_dir() {
            Ok(dir) => dir,
            Err(e) => {
                mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "封面缓存目录不可用,降级不缓存");
                return None;
            }
        };
        match store.cover_cache(dir, capacity).await {
            Ok(c) => Some(Arc::new(c)),
            Err(e) => {
                mineral_log::warn!(target: "cover", error = mineral_log::chain(&e), "封面缓存打开失败,降级不缓存");
                None
            }
        }
    }

    /// 禁用态 fetcher:不起 worker、不建 isahc client,纯 null object。
    ///
    /// 用于封面降级场景——headless / 无网 / isahc 建不起来(TLS / 证书),或测试里
    /// 不需要真抓图时。请求队列保持关闭，preview / decode 投递返回 `false`，
    /// `drain_ready()` 恒空。
    /// 与 [`CoverFetcher::spawn`] 不同,**不需要 tokio runtime**。
    pub(crate) fn disabled() -> Self {
        let (tx, rx) = mpsc::unbounded_channel::<CoverRequest>();
        drop(rx);
        Self {
            req_tx: tx,
            ready: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 投递一次真实低清 preview 请求。
    ///
    /// # Params:
    ///   - `source`: 来源，决定缓存子目录
    ///   - `url`: 图片源 URL
    ///   - `target`: 低清采样尺寸与主封面的格边策略
    ///
    /// # Return:
    ///   worker 队列仍可接收请求时返回 `true`
    pub(crate) fn preview(&self, source: SourceKind, url: MediaUrl, target: PreviewTarget) -> bool {
        self.req_tx
            .send(CoverRequest::Preview(PreviewRequest {
                source,
                url,
                target,
            }))
            .is_ok()
    }

    /// 投递一次完整图片解码请求。
    ///
    /// # Params:
    ///   - `source`: 来源，决定 Remote 缓存子目录
    ///   - `url`: 图片源 URL
    ///   - `pixels`: 当前配置的解码目标
    ///
    /// # Return:
    ///   worker 队列仍可接收请求时返回 `true`
    pub(crate) fn decode(
        &self,
        source: SourceKind,
        url: MediaUrl,
        pixels: CoverDecodePixelsConfig,
    ) -> bool {
        self.req_tx
            .send(CoverRequest::Decode(DecodeRequest {
                source,
                url,
                pixels,
            }))
            .is_ok()
    }

    /// 投递源图解码请求，通道只接收本次结果；队列关闭时接收端也会关闭。
    pub(crate) fn original(
        &self,
        source: SourceKind,
        url: MediaUrl,
    ) -> oneshot::Receiver<Result<RgbaImage, OriginalImageError>> {
        let (reply, result) = oneshot::channel();
        let _ = self
            .req_tx
            .send(CoverRequest::Original { source, url, reply });
        result
    }

    /// 把全部图片 worker completion 拿走。TUI 主循环 tick 调一次。
    pub(crate) fn drain_ready(&self) -> Vec<CoverCompletion> {
        std::mem::take(&mut *self.ready.lock())
    }
}

//! 封面请求的有限并发调度与完成结果收集。

use std::sync::Arc;
use std::time::Duration;

use crate::config::{CoverConfig, CoverDecodePixelsConfig};
use crate::persistence::TuiStore;
use futures_util::{StreamExt, stream};
use image::RgbaImage;
use isahc::HttpClient;
use isahc::config::Configurable;
use mineral_model::{MediaUrl, SourceKind};
use mineral_persist::CacheIndex;
use tokio::sync::{mpsc, oneshot};

use crate::image::key::PreviewTarget;

use super::types::{
    CoverCompletion, CoverRequest, DecodeRequest, OriginalImageError, PreviewRequest,
};
use super::worker::complete_request;

/// 共用下载、解码 worker 与源文件缓存。显示结果通过 `drain_ready` 收集，
/// 源图请求单独回复；禁用态关闭请求队列。
pub(crate) struct CoverFetcher {
    /// 待执行的请求。
    request_tx: mpsc::UnboundedSender<CoverRequest>,

    /// 由图片引擎独占消费的显示完成事件。
    completion_rx: mpsc::UnboundedReceiver<CoverCompletion>,
}

impl CoverFetcher {
    /// 启动后台调度，最多同时执行 `cfg.download_workers.max(1)` 个完整请求。
    /// 调用者须处于 Tokio runtime 中；HTTP 客户端构建失败时返回错误。
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
        let (request_tx, request_rx) = mpsc::unbounded_channel::<CoverRequest>();
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        let client = HttpClient::builder()
            .timeout(Duration::from_secs(*cfg.http_timeout_secs()))
            .build()?;
        // 磁盘缓存是优化项:store 不可用 / 目录解析失败不致命,降级成直连网络不缓存。
        let cache = Self::open_cache(store, cover_capacity).await;
        let concurrency = (*cfg.download_workers()).max(1);
        let cfg = Arc::new(cfg);
        tokio::spawn(async move {
            stream::unfold(request_rx, |mut request_rx| async move {
                request_rx.recv().await.map(|request| (request, request_rx))
            })
            .for_each_concurrent(concurrency, |request| {
                let client = &client;
                let cache = cache.as_ref();
                let cfg = &cfg;
                let completion_tx = &completion_tx;
                async move {
                    if let Some(completion) = complete_request(request, client, cache, cfg).await {
                        let _ = completion_tx.send(completion);
                    }
                }
            })
            .await;
        });
        Ok(Self {
            request_tx,
            completion_rx,
        })
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
        let (request_tx, request_rx) = mpsc::unbounded_channel();
        let (completion_tx, completion_rx) = mpsc::unbounded_channel();
        drop(request_rx);
        drop(completion_tx);
        Self {
            request_tx,
            completion_rx,
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
        self.request_tx
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
        self.request_tx
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
        let (reply_tx, reply_rx) = oneshot::channel();
        let _ = self.request_tx.send(CoverRequest::Original {
            source,
            url,
            reply_tx,
        });
        reply_rx
    }

    /// 把全部图片 worker completion 拿走。TUI 主循环 tick 调一次。
    pub(crate) fn drain_ready(&mut self) -> Vec<CoverCompletion> {
        let mut ready = Vec::new();
        while let Ok(completion) = self.completion_rx.try_recv() {
            ready.push(completion);
        }
        ready
    }
}

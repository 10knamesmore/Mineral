//! 网易读取端点的共享发送额度与每次调用独立的限流退避。

use std::future::Future;
use std::num::NonZeroU32;
use std::time::{Duration, Instant};

use governor::{DefaultDirectRateLimiter, Quota, RateLimiter};
use tokio::time::sleep;

use crate::Result;
use crate::config::RequestsConfig;

/// 端点策略随 channel 存活；不同专辑 ID 共用详情请求额度。
pub(crate) struct RequestPolicies {
    /// 专辑详情的发送额度和限流退避。
    pub(crate) album_detail: RequestPolicy,

    /// 两条歌词读取端点只退避，不限制发送速率。
    pub(crate) lyrics: RequestPolicy,
}

impl RequestPolicies {
    /// 根据来源配置创建本 channel 的请求策略。
    pub(crate) fn new(config: &RequestsConfig) -> Self {
        let quota = Quota::per_second(*config.album_detail_requests_per_second())
            .allow_burst(NonZeroU32::MIN);
        Self {
            album_detail: RequestPolicy {
                limiter: Some(RateLimiter::direct(quota)),
                retry_delays: config.retry_delays().clone(),
            },
            lyrics: RequestPolicy {
                limiter: None,
                retry_delays: config.retry_delays().clone(),
            },
        }
    }
}

/// 传输层按调用方传入的策略执行；重试步骤留在每次调用内部。
pub(crate) struct RequestPolicy {
    /// 本策略所有调用共享的额度；None 表示发送前无需等待额度。
    limiter: Option<DefaultDirectRateLimiter>,

    /// 限流失败后的等待序列；耗尽后返回最后一次原始错误。
    retry_delays: Vec<Duration>,
}

impl RequestPolicy {
    /// 在 HTTP 发送前申请额度；取消等待不会预订后续发送时间。
    pub(crate) async fn wait_until_ready(&self, endpoint: &str, attempt: usize) {
        let Some(limiter) = &self.limiter else {
            return;
        };
        if limiter.check().is_ok() {
            return;
        }
        let started = Instant::now();
        mineral_log::debug!(target: "channel_netease", endpoint, attempt, "waiting for request quota");
        limiter.until_ready().await;
        mineral_log::debug!(target: "channel_netease", endpoint, attempt,
            wait_ms = started.elapsed().as_millis(), "request quota acquired");
    }

    /// 仅重试网易限流业务码；每次尝试重新构造并发送单次请求。
    pub(crate) async fn retry<T, F, Fut>(&self, endpoint: &str, mut call: F) -> Result<T>
    where
        F: FnMut(usize) -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut delays = self.retry_delays.iter();
        let mut attempt = 1;
        loop {
            match call(attempt).await {
                Err(error) if error.is_rate_limited() => {
                    let Some(delay) = delays.next() else {
                        mineral_log::warn!(target: "channel_netease", endpoint, attempt,
                            error = mineral_log::chain(&error), "rate limit retries exhausted");
                        return Err(error);
                    };
                    mineral_log::warn!(target: "channel_netease", endpoint, attempt,
                        delay_ms = delay.as_millis(), error = mineral_log::chain(&error),
                        "rate limited; retrying after backoff");
                    sleep(*delay).await;
                    attempt += 1;
                }
                result => return result,
            }
        }
    }
}

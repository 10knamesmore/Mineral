//! 有效缓存直接返回；过期缓存先展示，再从远端更新。

use std::hash::{Hash, Hasher};

use async_stream::try_stream;
use chrono::{DateTime, TimeDelta, Utc};
use futures_util::stream::BoxStream;
use mineral_channel_core::FetchSnapshot;
use mineral_channel_core::store::{AlbumCacheEntry, NamespaceStore};
use mineral_model::{Album, AlbumId, CachePolicy};
use rustc_hash::FxHasher;

use crate::config::AlbumCacheConfig;
use crate::request::RequestPolicy;
use crate::transport::Transport;
use crate::{Result, api, convert};

/// 从持久缓存或远端加载专辑详情，仅保存来源配置的期限参数。
pub(crate) struct AlbumLoader {
    /// 基础缓存期限。
    ttl: TimeDelta,

    /// 期限两侧的随机增减上限。
    ttl_jitter: TimeDelta,
}

impl AlbumLoader {
    /// 根据配置创建随 channel 存活的加载器。
    pub(crate) fn new(config: &AlbumCacheConfig) -> Self {
        // 天数限定为 u32，构造期限与加上随机偏移均不会超出 TimeDelta 范围。
        Self {
            ttl: TimeDelta::days(i64::from(*config.ttl_days())),
            ttl_jitter: TimeDelta::days(i64::from(*config.ttl_jitter_days())),
        }
    }

    /// UseCache 的存储错误记日志后继续；Reload 写入失败透传，远端失败不续期。
    pub(crate) fn load<'a>(
        &'a self,
        transport: &'a Transport,
        policy: &'a RequestPolicy,
        persist: Option<&'a dyn NamespaceStore>,
        id: &'a AlbumId,
        cache: CachePolicy,
    ) -> BoxStream<'a, Result<FetchSnapshot<Album>>> {
        Box::pin(try_stream! {
            if let (CachePolicy::UseCache, Some(store)) = (cache, persist) {
                match store.get_album_cache(id).await {
                    Ok(Some(entry)) if entry.expired_at > Utc::now() => {
                        mineral_log::debug!(target: "channel_netease", album = %id,
                            expired_at = %entry.expired_at, "album cache hit");
                        yield FetchSnapshot::Fresh(entry.album);
                        return;
                    }
                    Ok(Some(entry)) => {
                        mineral_log::debug!(target: "channel_netease", album = %id,
                            expired_at = %entry.expired_at, "album cache expired");
                        yield FetchSnapshot::Cached(entry.album);
                    }
                    Ok(None) => {
                        mineral_log::debug!(target: "channel_netease", album = %id, "album cache miss");
                    }
                    Err(error) => {
                        mineral_log::warn!(target: "channel_netease", album = %id,
                            error = mineral_log::chain(&error), "album cache read failed");
                    }
                }
            }
            let dto = api::album::detail(transport, policy, id).await?;
            let loaded_at = Utc::now();
            let lifetime = self.lifetime(id, loaded_at);
            let entry = AlbumCacheEntry {
                album: convert::album_detail_to_model(dto)?,
                expired_at: loaded_at + lifetime,
            };
            if let Some(store) = persist {
                match store.put_album_cache(&entry).await {
                    Ok(()) => {}
                    Err(error) if cache == CachePolicy::Reload => Err(error)?,
                    Err(error) => {
                        mineral_log::warn!(target: "channel_netease", album = %id,
                            error = mineral_log::chain(&error), "album cache write failed");
                    }
                }
            }
            mineral_log::debug!(target: "channel_netease", album = %id,
                tracks = entry.album.tracks.len(), expired_at = %entry.expired_at,
                lifetime_ms = lifetime.num_milliseconds(), "album detail loaded");
            yield FetchSnapshot::Fresh(entry.album);
        })
    }

    /// 抓取完成时计算一次偏移，生成随快照保存的过期时间。
    fn lifetime(&self, id: &AlbumId, loaded_at: DateTime<Utc>) -> TimeDelta {
        let mut hasher = FxHasher::default();
        (id.namespace().name(), id.value(), loaded_at).hash(&mut hasher);
        let jitter_ms = self.ttl_jitter.num_milliseconds();
        let range = (jitter_ms * 2 + 1) as u64;
        let offset = (hasher.finish() % range) as i64 - jitter_ms;
        self.ttl + TimeDelta::milliseconds(offset)
    }
}

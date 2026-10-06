//! 在缓存缺失或到期时获取详情；缓存命中不占用远端请求额度。

use std::hash::{Hash, Hasher};

use chrono::{DateTime, TimeDelta, Utc};
use mineral_channel_core::store::{AlbumCacheEntry, NamespaceStore};
use mineral_model::{Album, AlbumId};
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

    /// 缓存读写失败记录日志，仍返回成功取得的远端详情；远端失败不续期。
    pub(crate) async fn load(
        &self,
        transport: &Transport,
        policy: &RequestPolicy,
        persist: Option<&dyn NamespaceStore>,
        id: &AlbumId,
    ) -> Result<Album> {
        if let Some(store) = persist {
            match store.get_album_cache(id).await {
                Ok(Some(entry)) if entry.expired_at > Utc::now() => {
                    mineral_log::debug!(target: "channel_netease", album = %id,
                        expired_at = %entry.expired_at, "album cache hit");
                    return Ok(entry.album);
                }
                Ok(Some(entry)) => {
                    mineral_log::debug!(target: "channel_netease", album = %id,
                        expired_at = %entry.expired_at, "album cache expired");
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
            album: convert::album_detail_to_model(dto),
            expired_at: loaded_at + lifetime,
        };
        if let Some(store) = persist
            && let Err(error) = store.put_album_cache(&entry).await
        {
            mineral_log::warn!(target: "channel_netease", album = %id,
                error = mineral_log::chain(&error), "album cache write failed");
        }
        mineral_log::debug!(target: "channel_netease", album = %id,
            tracks = entry.album.tracks.len(), expired_at = %entry.expired_at,
            lifetime_ms = lifetime.num_milliseconds(), "album detail loaded");
        Ok(entry.album)
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

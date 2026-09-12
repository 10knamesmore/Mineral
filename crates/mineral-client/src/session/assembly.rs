//! 按订阅和版本有界组装分片，合并播放队列与下载明细载荷。

use mineral_protocol::{SubscriptionId, UpdateEnvelope, UpdatePayload, assembly_limits};

/// 分片组装器:按 `(subscription, version)` 收齐 `parts` 后合并成一条逻辑更新。
#[derive(Default)]
pub(super) struct Assembler {
    /// 进行中的组装组(插入序用于超限时驱逐最旧)。
    groups: Vec<(SubscriptionId, u64, Group)>,
}

/// 一个组装组。
#[derive(Default)]
struct Group {
    /// 各片载荷(按 index 就位)。
    parts: Vec<Option<UpdatePayload>>,

    /// 已到片数。
    received: u32,

    /// 载荷字节估算。
    bytes: usize,
}

impl Assembler {
    /// 丢弃该订阅下所有版本更早的未完成组;返回是否有组被丢弃。
    ///
    /// # Params:
    ///   - `subscription`: 目标订阅
    ///   - `version`: 新到的版本
    fn drop_superseded(&mut self, subscription: SubscriptionId, version: u64) -> bool {
        let before = self.groups.len();
        self.groups
            .retain(|(id, existing, _group)| !(*id == subscription && *existing < version));
        before != self.groups.len()
    }

    /// 接收一条更新信封;完整时返回合并后的 `(subscription, version, payload)`。
    pub(super) fn accept(
        &mut self,
        envelope: UpdateEnvelope,
    ) -> Result<Option<(SubscriptionId, u64, UpdatePayload)>, String> {
        // 新版本到来 = 旧版本的未完成组再也不会补齐;丢掉并显式报告中断。
        let superseded = self.drop_superseded(envelope.subscription, envelope.version);
        if envelope.parts == 1 {
            return Ok(Some((
                envelope.subscription,
                envelope.version,
                envelope.payload,
            )));
        }
        if superseded {
            return Err("更早版本的未完成分片组已被取代".to_owned());
        }
        if envelope.parts > assembly_limits::MAX_PARTS {
            return Err(format!("分片数 {} 超限", envelope.parts));
        }
        if envelope.index >= envelope.parts {
            return Err(format!(
                "分片序号 {} 超出总片数 {}",
                envelope.index, envelope.parts
            ));
        }
        let key = (envelope.subscription, envelope.version);
        let position = self
            .groups
            .iter()
            .position(|(id, version, _group)| (*id, *version) == key);
        let position = match position {
            Some(position) => position,
            None => {
                if self.groups.len() >= assembly_limits::MAX_GROUPS {
                    // 驱逐最旧组:旧版本不会再被补发,直接放弃并请求重同步。
                    let evicted = self.groups.remove(0);
                    mineral_log::warn!(
                        target: "ipc",
                        subscription = evicted.0.value(),
                        version = evicted.1,
                        "组装组超限,放弃最旧组"
                    );
                }
                let total = usize::try_from(envelope.parts).unwrap_or(usize::MAX);
                self.groups.push((
                    key.0,
                    key.1,
                    Group {
                        parts: vec![None; total],
                        received: 0,
                        bytes: 0,
                    },
                ));
                self.groups.len().saturating_sub(1)
            }
        };
        let index = usize::try_from(envelope.index).unwrap_or(usize::MAX);
        let too_large = {
            let Some((_id, _version, group)) = self.groups.get_mut(position) else {
                return Err("组装组下标越界".to_owned());
            };
            if group.parts.get(index).is_some_and(Option::is_some) {
                return Err(format!("分片 {} 重复到达", envelope.index));
            }
            group.bytes = group
                .bytes
                .saturating_add(estimate_bytes(&envelope.payload));
            if group.bytes > assembly_limits::MAX_GROUP_BYTES {
                true
            } else {
                if let Some(slot) = group.parts.get_mut(index) {
                    *slot = Some(envelope.payload);
                } else {
                    return Err(format!("分片 {} 无处存放", envelope.index));
                }
                group.received = group.received.saturating_add(1);
                false
            }
        };
        if too_large {
            let bytes = self
                .groups
                .get(position)
                .map_or(0, |(_, _, group)| group.bytes);
            self.groups.remove(position);
            return Err(format!("组装字节 {bytes} 超限"));
        }
        let complete = self
            .groups
            .get(position)
            .is_some_and(|(_, _, group)| group.received >= envelope.parts);
        if !complete {
            return Ok(None);
        }
        let (_id, version, group) = self.groups.remove(position);
        let payload = merge_parts(group.parts)?;
        Ok(Some((envelope.subscription, version, payload)))
    }
}

/// 合并一组分片载荷。
fn merge_parts(parts: Vec<Option<UpdatePayload>>) -> Result<UpdatePayload, String> {
    let mut parts = parts
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "组装完成但存在缺片".to_owned())?;
    if parts.len() == 1 {
        return parts.pop().ok_or_else(|| "组装完成但载荷为空".to_owned());
    }
    let first = parts.first().cloned();
    match first {
        Some(UpdatePayload::Player {
            mut sync,
            queue_parts,
        }) => {
            if queue_parts == 0 {
                return Err("播放头段声明无队列分片却出现多片".to_owned());
            }
            let mut queue: Vec<mineral_model::Song> = Vec::new();
            let mut original_queue = None;
            let mut chunks: Vec<(
                u32,
                Vec<mineral_model::Song>,
                Option<Vec<mineral_model::Song>>,
            )> = Vec::new();
            for payload in parts.drain(1..) {
                match payload {
                    UpdatePayload::PlayerQueuePart {
                        offset,
                        queue,
                        original_queue: original,
                    } => {
                        if original.is_some() {
                            original_queue = original;
                        }
                        chunks.push((offset, queue, None));
                    }
                    other => {
                        return Err(format!("播放分片组出现意外载荷 {other:?}"));
                    }
                }
            }
            chunks.sort_by_key(|(offset, _, _)| *offset);
            for (_, chunk, _) in chunks {
                queue.extend(chunk);
            }
            sync.queue = Some(mineral_protocol::QueueSync {
                queue,
                original_queue,
            });
            Ok(UpdatePayload::Player { sync, queue_parts })
        }
        Some(UpdatePayload::DownloadsDetailHead { .. }) => {
            let mut rows = Vec::new();
            let mut chunks: Vec<(u32, Vec<mineral_protocol::SongDownloadView>)> = Vec::new();
            for payload in parts.drain(1..) {
                match payload {
                    UpdatePayload::DownloadsDetailPart { offset, rows: part } => {
                        chunks.push((offset, part));
                    }
                    other => {
                        return Err(format!("下载明细分片组出现意外载荷 {other:?}"));
                    }
                }
            }
            chunks.sort_by_key(|(offset, _)| *offset);
            for (_, chunk) in chunks {
                rows.extend(chunk);
            }
            Ok(UpdatePayload::DownloadsDetailSnapshot(rows))
        }
        Some(other) => Err(format!("不支持的组装载荷 {other:?}")),
        None => Err("组装完成但载荷为空".to_owned()),
    }
}

/// 载荷字节估算(只用于组装上限守卫,不要求精确)。
fn estimate_bytes(payload: &UpdatePayload) -> usize {
    match payload {
        UpdatePayload::Player { sync, .. } => sync
            .queue
            .as_ref()
            .map_or(256, |queue| queue.queue.len().saturating_mul(256)),
        UpdatePayload::PlayerQueuePart { queue, .. } => queue.len().saturating_mul(256),
        UpdatePayload::DownloadsDetailSnapshot(rows) => rows.len().saturating_mul(512),
        UpdatePayload::DownloadsDetailPart { rows, .. } => rows.len().saturating_mul(512),
        UpdatePayload::Pcm(chunk) => chunk.samples.len().saturating_mul(4),
        _ => 512,
    }
}

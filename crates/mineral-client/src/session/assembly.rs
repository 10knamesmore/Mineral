//! 按订阅和版本有界组装分片，合并播放队列与下载明细载荷。

use mineral_protocol::{SubscriptionId, UpdateEnvelope, UpdatePayload, assembly_limits};
use thiserror::Error;

/// 订阅分片无法安全合并的原因；调用方应请求完整重同步。
#[derive(Debug, Error)]
pub(super) enum AssemblyError {
    /// 新版本替代了尚未收齐的旧版本。
    #[error("旧版本分片组未完成")]
    Superseded,

    /// 本次声明的分片数超过限制。
    #[error("分片数 {parts} 超过上限 {max}")]
    TooManyParts {
        /// 收到的分片数。
        parts: u32,

        /// 允许的最大分片数。
        max: u32,
    },

    /// 分片序号不在声明范围内。
    #[error("分片序号 {index} 超出总片数 {parts}")]
    IndexOutOfBounds {
        /// 收到的序号。
        index: u32,

        /// 声明的分片数。
        parts: u32,
    },

    /// 同一序号重复到达。
    #[error("分片 {index} 重复到达")]
    Duplicate {
        /// 重复的分片序号。
        index: u32,
    },

    /// 累计分片大小超过限制。
    #[error("组装字节 {bytes} 超过上限 {max}")]
    TooLarge {
        /// 累计估算字节数。
        bytes: usize,

        /// 最大组装字节数。
        max: usize,
    },

    /// 已登记的组装组意外丢失。
    #[error("组装组下标 {index} 不存在")]
    MissingGroup {
        /// 预期的组装组位置。
        index: usize,
    },

    /// 合并时缺少某个分片。
    #[error("已收齐分片但存在空位")]
    MissingPart,

    /// 组装后的载荷为空。
    #[error("组装后的载荷为空")]
    EmptyPayload,

    /// 播放头段没有声明队列分片。
    #[error("播放头段未声明队列分片")]
    MissingQueueParts,

    /// 分片载荷类别与头段不匹配。
    #[error("分片载荷类别不符")]
    UnexpectedPayload,
}

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
    ) -> Result<Option<(SubscriptionId, u64, UpdatePayload)>, AssemblyError> {
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
            return Err(AssemblyError::Superseded);
        }
        if envelope.parts > assembly_limits::MAX_PARTS {
            return Err(AssemblyError::TooManyParts {
                parts: envelope.parts,
                max: assembly_limits::MAX_PARTS,
            });
        }
        if envelope.index >= envelope.parts {
            return Err(AssemblyError::IndexOutOfBounds {
                index: envelope.index,
                parts: envelope.parts,
            });
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
                return Err(AssemblyError::MissingGroup { index: position });
            };
            if group.parts.get(index).is_some_and(Option::is_some) {
                return Err(AssemblyError::Duplicate {
                    index: envelope.index,
                });
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
                    return Err(AssemblyError::IndexOutOfBounds {
                        index: envelope.index,
                        parts: envelope.parts,
                    });
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
            return Err(AssemblyError::TooLarge {
                bytes,
                max: assembly_limits::MAX_GROUP_BYTES,
            });
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
fn merge_parts(parts: Vec<Option<UpdatePayload>>) -> Result<UpdatePayload, AssemblyError> {
    let mut parts = parts
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(AssemblyError::MissingPart)?;
    if parts.len() == 1 {
        return parts.pop().ok_or(AssemblyError::EmptyPayload);
    }
    let first = parts.first().cloned();
    match first {
        Some(UpdatePayload::Player {
            mut sync,
            queue_parts,
        }) => {
            if queue_parts == 0 {
                return Err(AssemblyError::MissingQueueParts);
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
                    _ => return Err(AssemblyError::UnexpectedPayload),
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
                    _ => return Err(AssemblyError::UnexpectedPayload),
                }
            }
            chunks.sort_by_key(|(offset, _)| *offset);
            for (_, chunk) in chunks {
                rows.extend(chunk);
            }
            Ok(UpdatePayload::DownloadsDetailSnapshot(rows))
        }
        Some(_) => Err(AssemblyError::UnexpectedPayload),
        None => Err(AssemblyError::EmptyPayload),
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

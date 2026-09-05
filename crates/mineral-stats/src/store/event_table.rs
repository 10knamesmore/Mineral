//! 可计数、按时间裁剪并引用会话的事件实体。

use sea_orm::sea_query::{DynIden, Expr, ExprTrait, IntoIden, Query};
use sea_orm::{ConnectionTrait, EntityName};
use std::ops::Range;

use crate::entity;

/// 具有统一时间与会话字段的事件表。
#[derive(Clone, Copy, Debug)]
pub(crate) enum EventTable {
    /// searches 事件。
    Searches,
    /// seeks 事件。
    Seeks,
    /// pauses 事件。
    Pauses,
    /// volume_changes 事件。
    VolumeChanges,
    /// mode_changes 事件。
    ModeChanges,
    /// love_changes 事件。
    LoveChanges,
    /// queue_ops 事件。
    QueueOps,
    /// playlist_ops 事件。
    PlaylistOps,
    /// fetches 事件。
    Fetches,
    /// downloads 事件。
    Downloads,
    /// copy_renders 事件。
    CopyRenders,
    /// action_invocations 事件。
    ActionInvocations,
    /// config_overrides 事件。
    ConfigOverrides,
    /// store_writes 事件。
    StoreWrites,
    /// spawns 事件。
    Spawns,
    /// bus_messages 事件。
    BusMessages,
    /// fullscreen_changes 事件。
    FullscreenChanges,
    /// connection_rejects 事件。
    ConnectionRejects,
    /// client_connections 事件。
    ClientConnections,
    /// app_lifecycle 事件。
    AppLifecycle,
    /// stream_resolutions 事件。
    StreamResolutions,
    /// hook_fires 事件。
    HookFires,
    /// gapless_boundaries 事件。
    GaplessBoundaries,
    /// prefetches 事件。
    Prefetches,
    /// cache_harvests 事件。
    CacheHarvests,
    /// cache_evictions 事件。
    CacheEvictions,
    /// script_lifecycle 事件。
    ScriptLifecycle,
    /// config_reloads 事件。
    ConfigReloads,
}

/// 参与保留期清理和事件盘点的完整集合。
pub(crate) const EVENT_TABLES: &[EventTable] = &[
    EventTable::Searches,
    EventTable::Seeks,
    EventTable::Pauses,
    EventTable::VolumeChanges,
    EventTable::ModeChanges,
    EventTable::LoveChanges,
    EventTable::QueueOps,
    EventTable::PlaylistOps,
    EventTable::Fetches,
    EventTable::Downloads,
    EventTable::CopyRenders,
    EventTable::ActionInvocations,
    EventTable::ConfigOverrides,
    EventTable::StoreWrites,
    EventTable::Spawns,
    EventTable::BusMessages,
    EventTable::FullscreenChanges,
    EventTable::ConnectionRejects,
    EventTable::ClientConnections,
    EventTable::AppLifecycle,
    EventTable::StreamResolutions,
    EventTable::HookFires,
    EventTable::GaplessBoundaries,
    EventTable::Prefetches,
    EventTable::CacheHarvests,
    EventTable::CacheEvictions,
    EventTable::ScriptLifecycle,
    EventTable::ConfigReloads,
];

/// 从事件实体派生的公共列标识。
struct EventColumns {
    /// 数据表。
    table: DynIden,

    /// 事件发生时间。
    timestamp: DynIden,

    /// 关联会话。
    session_id: DynIden,
}

impl EventTable {
    /// 配置、日志与报表使用的稳定事件名称。
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Searches => entity::searches::Entity.table_name(),
            Self::Seeks => entity::seeks::Entity.table_name(),
            Self::Pauses => entity::pauses::Entity.table_name(),
            Self::VolumeChanges => entity::volume_changes::Entity.table_name(),
            Self::ModeChanges => entity::mode_changes::Entity.table_name(),
            Self::LoveChanges => entity::love_changes::Entity.table_name(),
            Self::QueueOps => entity::queue_ops::Entity.table_name(),
            Self::PlaylistOps => entity::playlist_ops::Entity.table_name(),
            Self::Fetches => entity::fetches::Entity.table_name(),
            Self::Downloads => entity::downloads::Entity.table_name(),
            Self::CopyRenders => entity::copy_renders::Entity.table_name(),
            Self::ActionInvocations => entity::action_invocations::Entity.table_name(),
            Self::ConfigOverrides => entity::config_overrides::Entity.table_name(),
            Self::StoreWrites => entity::store_writes::Entity.table_name(),
            Self::Spawns => entity::spawns::Entity.table_name(),
            Self::BusMessages => entity::bus_messages::Entity.table_name(),
            Self::FullscreenChanges => entity::fullscreen_changes::Entity.table_name(),
            Self::ConnectionRejects => entity::connection_rejects::Entity.table_name(),
            Self::ClientConnections => entity::client_connections::Entity.table_name(),
            Self::AppLifecycle => entity::app_lifecycle::Entity.table_name(),
            Self::StreamResolutions => entity::stream_resolutions::Entity.table_name(),
            Self::HookFires => entity::hook_fires::Entity.table_name(),
            Self::GaplessBoundaries => entity::gapless_boundaries::Entity.table_name(),
            Self::Prefetches => entity::prefetches::Entity.table_name(),
            Self::CacheHarvests => entity::cache_harvests::Entity.table_name(),
            Self::CacheEvictions => entity::cache_evictions::Entity.table_name(),
            Self::ScriptLifecycle => entity::script_lifecycle::Entity.table_name(),
            Self::ConfigReloads => entity::config_reloads::Entity.table_name(),
        }
    }

    /// 从实体取公共列，列引用由编译器检查。
    fn columns(self) -> EventColumns {
        match self {
            Self::Searches => EventColumns {
                table: entity::searches::Entity.into_iden(),
                timestamp: entity::searches::Column::Ts.into_iden(),
                session_id: entity::searches::Column::SessionId.into_iden(),
            },
            Self::Seeks => EventColumns {
                table: entity::seeks::Entity.into_iden(),
                timestamp: entity::seeks::Column::Ts.into_iden(),
                session_id: entity::seeks::Column::SessionId.into_iden(),
            },
            Self::Pauses => EventColumns {
                table: entity::pauses::Entity.into_iden(),
                timestamp: entity::pauses::Column::Ts.into_iden(),
                session_id: entity::pauses::Column::SessionId.into_iden(),
            },
            Self::VolumeChanges => EventColumns {
                table: entity::volume_changes::Entity.into_iden(),
                timestamp: entity::volume_changes::Column::Ts.into_iden(),
                session_id: entity::volume_changes::Column::SessionId.into_iden(),
            },
            Self::ModeChanges => EventColumns {
                table: entity::mode_changes::Entity.into_iden(),
                timestamp: entity::mode_changes::Column::Ts.into_iden(),
                session_id: entity::mode_changes::Column::SessionId.into_iden(),
            },
            Self::LoveChanges => EventColumns {
                table: entity::love_changes::Entity.into_iden(),
                timestamp: entity::love_changes::Column::Ts.into_iden(),
                session_id: entity::love_changes::Column::SessionId.into_iden(),
            },
            Self::QueueOps => EventColumns {
                table: entity::queue_ops::Entity.into_iden(),
                timestamp: entity::queue_ops::Column::Ts.into_iden(),
                session_id: entity::queue_ops::Column::SessionId.into_iden(),
            },
            Self::PlaylistOps => EventColumns {
                table: entity::playlist_ops::Entity.into_iden(),
                timestamp: entity::playlist_ops::Column::Ts.into_iden(),
                session_id: entity::playlist_ops::Column::SessionId.into_iden(),
            },
            Self::Fetches => EventColumns {
                table: entity::fetches::Entity.into_iden(),
                timestamp: entity::fetches::Column::Ts.into_iden(),
                session_id: entity::fetches::Column::SessionId.into_iden(),
            },
            Self::Downloads => EventColumns {
                table: entity::downloads::Entity.into_iden(),
                timestamp: entity::downloads::Column::Ts.into_iden(),
                session_id: entity::downloads::Column::SessionId.into_iden(),
            },
            Self::CopyRenders => EventColumns {
                table: entity::copy_renders::Entity.into_iden(),
                timestamp: entity::copy_renders::Column::Ts.into_iden(),
                session_id: entity::copy_renders::Column::SessionId.into_iden(),
            },
            Self::ActionInvocations => EventColumns {
                table: entity::action_invocations::Entity.into_iden(),
                timestamp: entity::action_invocations::Column::Ts.into_iden(),
                session_id: entity::action_invocations::Column::SessionId.into_iden(),
            },
            Self::ConfigOverrides => EventColumns {
                table: entity::config_overrides::Entity.into_iden(),
                timestamp: entity::config_overrides::Column::Ts.into_iden(),
                session_id: entity::config_overrides::Column::SessionId.into_iden(),
            },
            Self::StoreWrites => EventColumns {
                table: entity::store_writes::Entity.into_iden(),
                timestamp: entity::store_writes::Column::Ts.into_iden(),
                session_id: entity::store_writes::Column::SessionId.into_iden(),
            },
            Self::Spawns => EventColumns {
                table: entity::spawns::Entity.into_iden(),
                timestamp: entity::spawns::Column::Ts.into_iden(),
                session_id: entity::spawns::Column::SessionId.into_iden(),
            },
            Self::BusMessages => EventColumns {
                table: entity::bus_messages::Entity.into_iden(),
                timestamp: entity::bus_messages::Column::Ts.into_iden(),
                session_id: entity::bus_messages::Column::SessionId.into_iden(),
            },
            Self::FullscreenChanges => EventColumns {
                table: entity::fullscreen_changes::Entity.into_iden(),
                timestamp: entity::fullscreen_changes::Column::Ts.into_iden(),
                session_id: entity::fullscreen_changes::Column::SessionId.into_iden(),
            },
            Self::ConnectionRejects => EventColumns {
                table: entity::connection_rejects::Entity.into_iden(),
                timestamp: entity::connection_rejects::Column::Ts.into_iden(),
                session_id: entity::connection_rejects::Column::SessionId.into_iden(),
            },
            Self::ClientConnections => EventColumns {
                table: entity::client_connections::Entity.into_iden(),
                timestamp: entity::client_connections::Column::Ts.into_iden(),
                session_id: entity::client_connections::Column::SessionId.into_iden(),
            },
            Self::AppLifecycle => EventColumns {
                table: entity::app_lifecycle::Entity.into_iden(),
                timestamp: entity::app_lifecycle::Column::Ts.into_iden(),
                session_id: entity::app_lifecycle::Column::SessionId.into_iden(),
            },
            Self::StreamResolutions => EventColumns {
                table: entity::stream_resolutions::Entity.into_iden(),
                timestamp: entity::stream_resolutions::Column::Ts.into_iden(),
                session_id: entity::stream_resolutions::Column::SessionId.into_iden(),
            },
            Self::HookFires => EventColumns {
                table: entity::hook_fires::Entity.into_iden(),
                timestamp: entity::hook_fires::Column::Ts.into_iden(),
                session_id: entity::hook_fires::Column::SessionId.into_iden(),
            },
            Self::GaplessBoundaries => EventColumns {
                table: entity::gapless_boundaries::Entity.into_iden(),
                timestamp: entity::gapless_boundaries::Column::Ts.into_iden(),
                session_id: entity::gapless_boundaries::Column::SessionId.into_iden(),
            },
            Self::Prefetches => EventColumns {
                table: entity::prefetches::Entity.into_iden(),
                timestamp: entity::prefetches::Column::Ts.into_iden(),
                session_id: entity::prefetches::Column::SessionId.into_iden(),
            },
            Self::CacheHarvests => EventColumns {
                table: entity::cache_harvests::Entity.into_iden(),
                timestamp: entity::cache_harvests::Column::Ts.into_iden(),
                session_id: entity::cache_harvests::Column::SessionId.into_iden(),
            },
            Self::CacheEvictions => EventColumns {
                table: entity::cache_evictions::Entity.into_iden(),
                timestamp: entity::cache_evictions::Column::Ts.into_iden(),
                session_id: entity::cache_evictions::Column::SessionId.into_iden(),
            },
            Self::ScriptLifecycle => EventColumns {
                table: entity::script_lifecycle::Entity.into_iden(),
                timestamp: entity::script_lifecycle::Column::Ts.into_iden(),
                session_id: entity::script_lifecycle::Column::SessionId.into_iden(),
            },
            Self::ConfigReloads => EventColumns {
                table: entity::config_reloads::Entity.into_iden(),
                timestamp: entity::config_reloads::Column::Ts.into_iden(),
                session_id: entity::config_reloads::Column::SessionId.into_iden(),
            },
        }
    }

    /// 统计全部或指定时间窗口内的事件数量。
    pub(crate) async fn count(
        self,
        db: &impl ConnectionTrait,
        range: Option<Range<i64>>,
    ) -> color_eyre::Result<i64> {
        let columns = self.columns();
        let mut query = Query::select();
        query
            .expr(Expr::col(sea_orm::sea_query::Asterisk).count())
            .from(columns.table);
        if let Some(range) = range {
            query
                .and_where(Expr::col(columns.timestamp.clone()).gte(range.start))
                .and_where(Expr::col(columns.timestamp).lt(range.end));
        }
        let row = db
            .query_one(&query)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("事件计数未返回行 table={}", self.name()))?;
        Ok(row.try_get_by_index(/*index*/ 0)?)
    }

    /// 删除严格早于水位的事件。
    pub(crate) async fn delete_before(
        self,
        db: &impl ConnectionTrait,
        before: i64,
    ) -> color_eyre::Result<()> {
        let columns = self.columns();
        let query = Query::delete()
            .from_table(columns.table)
            .and_where(Expr::col(columns.timestamp).lt(before))
            .to_owned();
        db.execute(&query).await?;
        Ok(())
    }

    /// 要删除的会话不得仍有事件引用；预览时仅考虑水位后的存活事件。
    pub(crate) fn has_no_reference(self, surviving_since: Option<i64>) -> Expr {
        let columns = self.columns();
        let mut query = Query::select();
        query
            .expr(Expr::value(1))
            .from(columns.table.clone())
            .and_where(
                Expr::col((columns.table.clone(), columns.session_id))
                    .equals((entity::sessions::Entity, entity::sessions::Column::Id)),
            );
        if let Some(since) = surviving_since {
            query.and_where(Expr::col((columns.table, columns.timestamp)).gte(since));
        }
        Expr::exists(query).not()
    }
}

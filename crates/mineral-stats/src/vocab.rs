//! 受控词汇的持久化映射。
//!
//! SQLite 使用 snake_case 文本值，读写由 SeaORM 枚举映射完成。
//! 未知值作为解码错误返回。

/// 一次播放的结束原因(plays.finish_reason)。
///
/// 点播新歌顶掉在播曲时,在播曲结算记 [`FinishReason::Skip`]。
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    sea_orm::EnumIter,
    sea_orm::DeriveActiveEnum,
    serde::Serialize,
)]
#[sea_orm(rs_type = "String", db_type = "Text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// 自然播完。
    Eof,

    /// 用户跳过(next / prev 切歌,或点播顶掉)。
    Skip,

    /// 用户显式停止。
    Stop,

    /// 解码 / 取链失败导致中断。
    Error,
}

/// 播放当时音频本体的来源位置(plays.playback_origin)。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sea_orm::EnumIter, sea_orm::DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "Text", rename_all = "snake_case")]
pub enum PlaybackOrigin {
    /// 下载导出库(永久,文件系统即真相)。
    Download,

    /// 音频本体缓存(LRU,可被淘汰)。
    Cache,

    /// 远端流(可能边播边收割入缓存)。
    Remote,
}

/// 行为的发起方(跨切面列 actor)。
///
/// 脚本命令与用户请求共用同一播放核心,不带此标注则分不清 seek / love 是人按的
/// 还是脚本干的。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sea_orm::EnumIter, sea_orm::DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "Text", rename_all = "snake_case")]
pub enum Actor {
    /// 用户在 TUI 交互发起。
    User,

    /// Lua 脚本发起。
    Script,

    /// daemon 自治链路发起(自动接续 / 会话恢复等)。
    System,

    /// CLI 子命令发起。
    Cli,
}

/// 播放模式(plays.play_mode / mode_changes.from_mode / to_mode)。
///
/// 与 client 侧播放模式同构但独立定义——stats 保持 client 形态中立、不依赖 protocol,
/// 边界转换在 server 侧做;落库串与既有 `script_name` 词汇一致,历史数据不漂移。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sea_orm::EnumIter, sea_orm::DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "Text", rename_all = "snake_case")]
pub enum PlayMode {
    /// 顺序播放(到底停止)。
    Sequential,

    /// 随机播放。
    Shuffle,

    /// 整列循环。
    RepeatAll,

    /// 单曲循环。
    RepeatOne,
}

/// 一行播放的发起方式(plays.origin_kind)。
///
/// 与队列上下文([`crate::context::QueueContext`])分层:本枚举答「这一行怎么起
/// 播的」,上下文答「队列来自哪」。
#[derive(Clone, Copy, Debug, PartialEq, Eq, sea_orm::EnumIter, sea_orm::DeriveActiveEnum)]
#[sea_orm(rs_type = "String", db_type = "Text", rename_all = "snake_case")]
pub enum PlayOrigin {
    /// 用户在某视图显式点播。
    Explicit,

    /// 播完自动接续 / next / prev 推进。
    AutoAdvance,

    /// daemon 重启后的会话恢复起播。
    Resume,

    /// Lua 脚本发起。
    Script,

    /// 未标注(旧 client 缺省,向后兼容)。
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::{Actor, FinishReason, PlayOrigin, PlaybackOrigin};
    use sea_orm::{
        ActiveEnum, ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend,
        EntityTrait, QueryOrder, QuerySelect, Schema, Set, TryGetable,
    };

    /// 枚举存储格式验证使用的文本列。
    mod stored_value {
        use sea_orm::{
            ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
            EnumIter, PrimaryKeyTrait,
        };

        /// 一次枚举值写入。
        #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
        #[sea_orm(table_name = "enum_values")]
        pub struct Model {
            /// 插入顺序。
            #[sea_orm(primary_key)]
            pub id: i64,

            /// 枚举的文本表示。
            pub value: String,
        }

        /// 此测试表没有关联实体。
        #[derive(Clone, Copy, Debug, EnumIter, DeriveRelation)]
        pub enum Relation {}

        impl ActiveModelBehavior for ActiveModel {}
    }

    /// 在独立内存数据库里验证枚举的真实读写。
    async fn memory_database() -> color_eyre::Result<DatabaseConnection> {
        let mut options = ConnectOptions::new("sqlite::memory:");
        options.max_connections(/*value*/ 1);
        let db = Database::connect(options).await?;
        db.execute(&Schema::new(DbBackend::Sqlite).create_table_from_entity(stored_value::Entity))
            .await?;
        Ok(db)
    }

    /// 写入时检查落库文本，读取时使用同一枚举类型解码。
    async fn assert_round_trip<T>(cases: &[(T, &str)]) -> color_eyre::Result<()>
    where
        T: ActiveEnum<Value = String> + Copy + std::fmt::Debug + PartialEq + TryGetable,
    {
        let db = memory_database().await?;
        for (variant, text) in cases {
            stored_value::Entity::insert(stored_value::ActiveModel {
                value: Set(variant.to_value()),
                ..Default::default()
            })
            .exec(&db)
            .await?;
            let raw = stored_value::Entity::find()
                .select_only()
                .column(stored_value::Column::Value)
                .order_by_desc(stored_value::Column::Id)
                .into_tuple::<String>()
                .one(&db)
                .await?
                .ok_or_else(|| color_eyre::eyre::eyre!("missing stored enum"))?;
            assert_eq!(&raw, text, "落库串 for {variant:?}");
            let (got,) = stored_value::Entity::find()
                .select_only()
                .column(stored_value::Column::Value)
                .order_by_desc(stored_value::Column::Id)
                .into_tuple::<(T,)>()
                .one(&db)
                .await?
                .ok_or_else(|| color_eyre::eyre::eyre!("missing decoded enum"))?;
            assert_eq!(got, *variant);
        }
        Ok(())
    }

    #[tokio::test]
    async fn finish_reason_round_trips() -> color_eyre::Result<()> {
        assert_round_trip(&[
            (FinishReason::Eof, "eof"),
            (FinishReason::Skip, "skip"),
            (FinishReason::Stop, "stop"),
            (FinishReason::Error, "error"),
        ])
        .await
    }

    #[tokio::test]
    async fn playback_origin_round_trips() -> color_eyre::Result<()> {
        assert_round_trip(&[
            (PlaybackOrigin::Download, "download"),
            (PlaybackOrigin::Cache, "cache"),
            (PlaybackOrigin::Remote, "remote"),
        ])
        .await
    }

    #[tokio::test]
    async fn actor_round_trips() -> color_eyre::Result<()> {
        assert_round_trip(&[
            (Actor::User, "user"),
            (Actor::Script, "script"),
            (Actor::System, "system"),
            (Actor::Cli, "cli"),
        ])
        .await
    }

    #[tokio::test]
    async fn play_origin_round_trips() -> color_eyre::Result<()> {
        assert_round_trip(&[
            (PlayOrigin::Explicit, "explicit"),
            (PlayOrigin::AutoAdvance, "auto_advance"),
            (PlayOrigin::Resume, "resume"),
            (PlayOrigin::Script, "script"),
            (PlayOrigin::Unknown, "unknown"),
        ])
        .await
    }

    #[tokio::test]
    async fn play_mode_round_trips() -> color_eyre::Result<()> {
        use super::PlayMode;
        assert_round_trip(&[
            (PlayMode::Sequential, "sequential"),
            (PlayMode::Shuffle, "shuffle"),
            (PlayMode::RepeatAll, "repeat_all"),
            (PlayMode::RepeatOne, "repeat_one"),
        ])
        .await
    }
}

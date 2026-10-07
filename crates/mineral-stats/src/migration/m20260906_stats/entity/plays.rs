//! m20260906_stats 建表时的 plays 实体快照。

use sea_orm::{
    ActiveModelBehavior, DeriveEntityModel, DerivePrimaryKey, DeriveRelation, EntityTrait,
    EnumIter, PrimaryKeyTrait,
};

/// 一条完整的数据库记录。
#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "plays")]
pub struct Model {
    /// 记录身份。
    #[sea_orm(primary_key)]
    #[sea_orm(column_type = "Integer", nullable)]
    pub id: i64,

    /// 来源稳定名。
    #[sea_orm(column_type = "Text")]
    pub ns: String,

    /// 来源内歌曲身份。
    #[sea_orm(column_type = "Text")]
    pub song_value: String,

    /// 开始时间，Unix 毫秒。
    #[sea_orm(column_type = "Integer")]
    pub started_at: i64,

    /// 结束时间，Unix 毫秒。
    #[sea_orm(column_type = "Integer")]
    pub ended_at: i64,

    /// 实际收听毫秒数。
    #[sea_orm(column_type = "Integer")]
    pub listen_ms: i64,

    /// 播放开始时已知的时长，单位毫秒。
    #[sea_orm(column_type = "Integer")]
    pub duration_ms_snapshot: Option<i64>,

    /// 播放结束原因。
    #[sea_orm(column_type = "Text")]
    pub finish_reason: String,

    /// 跳过时的播放进度，单位毫秒。
    #[sea_orm(column_type = "Integer")]
    pub skip_at_ms: Option<i64>,

    /// 播放模式。
    #[sea_orm(column_type = "Text")]
    pub play_mode: String,

    /// 所属会话身份。
    #[sea_orm(column_type = "Integer")]
    pub session_id: i64,

    /// 播放发起方式。
    #[sea_orm(column_type = "Text")]
    pub origin_kind: String,

    /// 行为发起方。
    #[sea_orm(column_type = "Text")]
    pub actor: String,

    /// 队列上下文类型。
    #[sea_orm(column_type = "Text")]
    pub context_kind: String,

    /// 队列上下文身份。
    #[sea_orm(column_type = "Text")]
    pub context_ref: Option<String>,

    /// 实际音频格式。
    #[sea_orm(column_type = "Text")]
    pub audio_format: Option<String>,

    /// 是否无损音频。
    #[sea_orm(column_type = "Integer")]
    pub is_lossless: Option<i64>,

    /// 实际码率，单位 bit/s。
    #[sea_orm(column_type = "Integer")]
    pub bitrate_bps: Option<i64>,

    /// 发起资源请求时的音质档位。
    #[sea_orm(column_type = "Text")]
    pub quality: Option<String>,

    /// 采样位深。
    #[sea_orm(column_type = "Integer")]
    pub bit_depth: Option<i64>,

    /// 音频资源来源位置。
    #[sea_orm(column_type = "Text")]
    pub playback_origin: String,

    /// 是否使用替代资源。
    #[sea_orm(column_type = "Integer")]
    pub substituted: i64,

    /// 队列上下文名称。
    #[sea_orm(column_type = "Text")]
    pub context_name: Option<String>,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    /// 播放所属会话。
    #[sea_orm(
        belongs_to = "super::sessions::Entity",
        from = "Column::SessionId",
        to = "super::sessions::Column::Id",
        on_update = "NoAction",
        on_delete = "NoAction"
    )]
    Session,
}

impl ActiveModelBehavior for ActiveModel {}

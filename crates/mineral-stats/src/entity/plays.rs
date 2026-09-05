//! plays 的持久化实体。

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
    pub id: i64,

    /// 来源稳定名。
    pub ns: String,

    /// 来源内歌曲身份。
    pub song_value: String,

    /// 开始时间，Unix 毫秒。
    pub started_at: i64,

    /// 结束时间，Unix 毫秒。
    pub ended_at: i64,

    /// 实际收听毫秒数。
    pub listen_ms: i64,

    /// 播放开始时已知的时长，单位毫秒。
    pub duration_ms_snapshot: Option<i64>,

    /// 播放结束原因。
    pub finish_reason: crate::FinishReason,

    /// 跳过时的播放进度，单位毫秒。
    pub skip_at_ms: Option<i64>,

    /// 播放模式。
    pub play_mode: crate::PlayMode,

    /// 所属会话身份。
    pub session_id: i64,

    /// 播放发起方式。
    pub origin_kind: crate::PlayOrigin,

    /// 行为发起方。
    pub actor: crate::Actor,

    /// 队列上下文类型。
    pub context_kind: String,

    /// 队列上下文身份。
    pub context_ref: Option<String>,

    /// 实际音频格式。
    pub audio_format: Option<String>,

    /// 是否无损音频。
    pub is_lossless: Option<i64>,

    /// 实际码率，单位 bit/s。
    pub bitrate_bps: Option<i64>,

    /// 音质标识。
    pub quality: Option<String>,

    /// 采样位深。
    pub bit_depth: Option<i64>,

    /// 音频资源来源位置。
    pub playback_origin: crate::PlaybackOrigin,

    /// 是否使用替代资源。
    pub substituted: i64,

    /// 队列上下文名称。
    pub context_name: Option<String>,
}

/// 数据库声明的实体关系。
#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}

//! plays 的初始结构快照。

use sea_orm::sea_query::{
    self, ColumnDef, Expr, ExprTrait, ForeignKey, ForeignKeyAction, Iden, Index,
    IndexCreateStatement, Table, TableCreateStatement,
};

/// 本版迁移使用的表和列标识。
#[derive(Iden)]
pub(in crate::migration) enum Plays {
    /// 数据表。
    Table,
    /// 记录身份。
    Id,
    /// 来源稳定名。
    Ns,
    /// 来源内歌曲身份。
    SongValue,
    /// 开始时间，Unix 毫秒。
    StartedAt,
    /// 结束时间，Unix 毫秒。
    EndedAt,
    /// 实际收听毫秒数。
    ListenMs,
    /// 播放开始时已知的时长，单位毫秒。
    DurationMsSnapshot,
    /// 播放结束原因。
    FinishReason,
    /// 跳过时的播放进度，单位毫秒。
    SkipAtMs,
    /// 播放模式。
    PlayMode,
    /// 所属会话身份。
    SessionId,
    /// 播放发起方式。
    OriginKind,
    /// 行为发起方。
    Actor,
    /// 队列上下文类型。
    ContextKind,
    /// 队列上下文身份。
    ContextRef,
    /// 实际音频格式。
    AudioFormat,
    /// 是否无损音频。
    IsLossless,
    /// 实际码率，单位 bit/s。
    BitrateBps,
    /// 音质标识。
    Quality,
    /// 采样位深。
    BitDepth,
    /// 音频资源来源位置。
    PlaybackOrigin,
    /// 是否使用替代资源。
    Substituted,
    /// 队列上下文名称。
    ContextName,
}

/// 创建此版本的表及其完整约束。
pub(in crate::migration) fn definition() -> TableCreateStatement {
    let mut table = Table::create();
    table.table(Plays::Table);
    table.col(
        ColumnDef::new(Plays::Id)
            .integer()
            .primary_key()
            .auto_increment(),
    );
    table.col(ColumnDef::new(Plays::Ns).text().not_null());
    table.col(ColumnDef::new(Plays::SongValue).text().not_null());
    table.col(ColumnDef::new(Plays::StartedAt).integer().not_null());
    table.col(ColumnDef::new(Plays::EndedAt).integer().not_null());
    table.col(ColumnDef::new(Plays::ListenMs).integer().not_null());
    table.col(ColumnDef::new(Plays::DurationMsSnapshot).integer());
    table.col(ColumnDef::new(Plays::FinishReason).text().not_null());
    table.col(ColumnDef::new(Plays::SkipAtMs).integer());
    table.col(ColumnDef::new(Plays::PlayMode).text().not_null());
    table.col(ColumnDef::new(Plays::SessionId).integer().not_null());
    table.col(ColumnDef::new(Plays::OriginKind).text().not_null());
    table.col(ColumnDef::new(Plays::Actor).text().not_null());
    table.col(ColumnDef::new(Plays::ContextKind).text().not_null());
    table.col(ColumnDef::new(Plays::ContextRef).text());
    table.col(ColumnDef::new(Plays::AudioFormat).text());
    table.col(ColumnDef::new(Plays::IsLossless).integer());
    table.col(ColumnDef::new(Plays::BitrateBps).integer());
    table.col(ColumnDef::new(Plays::Quality).text());
    table.col(ColumnDef::new(Plays::BitDepth).integer());
    table.col(ColumnDef::new(Plays::PlaybackOrigin).text().not_null());
    table.col(ColumnDef::new(Plays::Substituted).integer().not_null());
    table.col(ColumnDef::new(Plays::ContextName).text());
    table.check(Expr::col(Plays::FinishReason).is_in(["eof", "skip", "stop", "error"]));
    table.check(Expr::col(Plays::PlayMode).is_in([
        "sequential",
        "shuffle",
        "repeat_all",
        "repeat_one",
    ]));
    table.check(Expr::col(Plays::OriginKind).is_in([
        "explicit",
        "auto_advance",
        "resume",
        "script",
        "unknown",
    ]));
    table.check(Expr::col(Plays::Actor).is_in(["user", "script", "system", "cli"]));
    table.check(
        Expr::col(Plays::ContextKind)
            .is_in(["search", "playlist", "album", "artist", "manual", "unknown"]),
    );
    table.check(
        Expr::col(Plays::Quality).is_in(["standard", "higher", "exhigh", "lossless", "hires"]),
    );
    table.check(Expr::col(Plays::PlaybackOrigin).is_in(["download", "cache", "remote"]));
    table.foreign_key(
        ForeignKey::create()
            .from(Plays::Table, Plays::SessionId)
            .to(
                super::sessions::Sessions::Table,
                super::sessions::Sessions::Id,
            )
            .on_update(ForeignKeyAction::NoAction)
            .on_delete(ForeignKeyAction::NoAction),
    );
    table
}

/// 为此表建立业务查询使用的索引。
pub(in crate::migration) fn indexes() -> Vec<IndexCreateStatement> {
    vec![
        Index::create()
            .name("idx_plays_context")
            .table(Plays::Table)
            .col(Plays::ContextKind)
            .col(Plays::ContextRef)
            .to_owned(),
        Index::create()
            .name("idx_plays_session")
            .table(Plays::Table)
            .col(Plays::SessionId)
            .to_owned(),
        Index::create()
            .name("idx_plays_song")
            .table(Plays::Table)
            .col(Plays::Ns)
            .col(Plays::SongValue)
            .col(Plays::StartedAt)
            .to_owned(),
        Index::create()
            .name("idx_plays_started")
            .table(Plays::Table)
            .col(Plays::StartedAt)
            .to_owned(),
    ]
}

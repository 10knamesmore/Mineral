//! plays / sessions 事实行的写入。

use crate::entity::sessions;
use color_eyre::eyre::WrapErr as _;
use mineral_model::AudioFormat;
use sea_orm::sea_query::Expr;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::play::PlayRecord;
use crate::store::StatsStore;

impl StatsStore {
    /// 开一行新收听会话,返回其 id。降级时不写、返回 `None`。
    ///
    /// # Params:
    ///   - `started_at`: 会话起始 epoch ms(初始 `ended_at` 同值)
    pub async fn open_session(&self, started_at: i64) -> color_eyre::Result<Option<i64>> {
        let Some(pool) = self.pool() else {
            return Ok(None);
        };
        let id = crate::entity::sessions::Entity::insert(crate::entity::sessions::ActiveModel {
            started_at: Set(started_at),
            ended_at: Set(started_at),
            ..Default::default()
        })
        .exec(pool)
        .await
        .wrap_err("open_session 落库失败")?
        .last_insert_id;
        Ok(Some(id))
    }

    /// 随播放活动推进,更新会话结束时刻。降级时 no-op。
    pub async fn touch_session(&self, session_id: i64, ended_at: i64) -> color_eyre::Result<()> {
        let Some(db) = self.pool() else {
            return Ok(());
        };
        sessions::Entity::update_many()
            .col_expr(sessions::Column::EndedAt, Expr::value(ended_at))
            .filter(sessions::Column::Id.eq(session_id))
            .exec(db)
            .await
            .wrap_err("touch_session 落库失败")?;
        Ok(())
    }

    /// 落一行播放事实。降级时静默 no-op。
    ///
    /// `is_lossless` 由 `audio_format` 现算(不作单独字段,避免第二数据源);ID 拆成
    /// `ns` + 裸 `song_value` 两列;上下文拆成 `context_kind` + `context_ref`。
    pub async fn record_play(&self, rec: &PlayRecord) -> color_eyre::Result<()> {
        let Some(pool) = self.pool() else {
            return Ok(());
        };
        let ns = rec.song_id.namespace().name();
        let song_value = rec.song_id.value();
        let (context_kind, context_ref) = rec.context.to_columns();
        let context_ref = context_ref.as_deref();
        let context_name = rec.context.display_name();
        let audio_format = rec.audio.audio_format.as_ref().map(AudioFormat::as_str);
        let is_lossless = rec
            .audio
            .audio_format
            .as_ref()
            .map(|f| i64::from(f.is_lossless()));
        let quality = rec.audio.quality.map(|q| q.as_str());
        let substituted = i64::from(rec.audio.substituted);
        crate::entity::plays::Entity::insert(crate::entity::plays::ActiveModel {
            ns: Set(ns.to_owned()),
            song_value: Set(song_value.to_owned()),
            started_at: Set(rec.started_at),
            ended_at: Set(rec.ended_at),
            listen_ms: Set(rec.listen_ms),
            duration_ms_snapshot: Set(rec.duration_ms_snapshot),
            finish_reason: Set(rec.finish_reason),
            skip_at_ms: Set(rec.skip_at_ms),
            play_mode: Set(rec.play_mode),
            session_id: Set(rec.session_id),
            origin_kind: Set(rec.origin),
            actor: Set(rec.actor),
            context_kind: Set(context_kind.to_owned()),
            context_ref: Set(context_ref.map(str::to_owned)),
            context_name: Set(context_name.map(str::to_owned)),
            audio_format: Set(audio_format.map(str::to_owned)),
            is_lossless: Set(is_lossless),
            bitrate_bps: Set(rec.audio.bitrate_bps),
            quality: Set(quality.map(str::to_owned)),
            bit_depth: Set(rec.audio.bit_depth),
            playback_origin: Set(rec.playback_origin),
            substituted: Set(substituted),
            ..Default::default()
        })
        .exec(pool)
        .await
        .wrap_err_with(|| format!("record_play 落库失败 song={song_value}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::context::QueueContext;
    use crate::play::PlayRecord;
    use crate::store::StatsStore;
    use crate::vocab::{Actor, FinishReason, PlayOrigin, PlaybackOrigin};
    use mineral_model::{AudioFormat, BitRate, PlaylistId, SongId, SourceKind};
    use sea_orm::sea_query::ExprTrait;
    use sea_orm::{EntityTrait, QueryFilter, QuerySelect};

    /// 读回断言使用的字段投影，包含类型化的枚举值。
    #[derive(sea_orm::FromQueryResult)]
    struct PlayRow {
        /// 来源 name。
        ns: String,
        /// 裸歌曲 id。
        song_value: String,
        /// 实际收听 ms。
        listen_ms: i64,
        /// 结束原因(TEXT → 枚举)。
        finish_reason: FinishReason,
        /// 播放模式串。
        play_mode: String,
        /// 会话 id。
        session_id: i64,
        /// 发起方式(TEXT → 枚举)。
        origin_kind: PlayOrigin,
        /// 发起方(TEXT → 枚举)。
        actor: Actor,
        /// 上下文 kind。
        context_kind: String,
        /// 上下文 ref。
        context_ref: Option<String>,
        /// 上下文显示名快照。
        context_name: Option<String>,
        /// 格式串。
        audio_format: Option<String>,
        /// 无损标记(现算,0/1)。
        is_lossless: Option<i64>,
        /// 来源位置(TEXT → 枚举)。
        playback_origin: PlaybackOrigin,
        /// 顶换标记(0/1)。
        substituted: i64,
    }

    async fn open_temp() -> color_eyre::Result<(tempfile::TempDir, StatsStore)> {
        let dir = tempfile::tempdir()?;
        let store = StatsStore::open(&dir.path().join("stats.db")).await?;
        Ok((dir, store))
    }

    fn sample(session_id: i64) -> PlayRecord {
        PlayRecord {
            song_id: SongId::new(SourceKind::NETEASE, "42"),
            started_at: 1000,
            ended_at: 4000,
            listen_ms: 3000,
            duration_ms_snapshot: Some(200_000),
            finish_reason: FinishReason::Eof,
            skip_at_ms: None,
            play_mode: crate::PlayMode::RepeatOne,
            session_id,
            origin: PlayOrigin::Explicit,
            actor: Actor::User,
            context: QueueContext::Playlist {
                id: PlaylistId::new(SourceKind::NETEASE, "7"),
                name: Some("收藏夹".to_owned()),
            },
            audio: crate::PlayAudioSnapshot {
                audio_format: Some(AudioFormat::Flac),
                bitrate_bps: Some(900_000),
                quality: Some(BitRate::Lossless),
                bit_depth: Some(24),
                substituted: false,
            },
            playback_origin: PlaybackOrigin::Download,
        }
    }

    #[tokio::test]
    async fn record_play_round_trips_all_columns() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(1000)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected session id"))?;
        store.record_play(&sample(sid)).await?;
        let pool = store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected live pool"))?;
        let row = crate::entity::plays::Entity::find()
            .select_only()
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::Ns,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::SongValue,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::ListenMs,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::FinishReason,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::PlayMode,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::SessionId,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::OriginKind,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::Actor,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::ContextKind,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::ContextRef,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::ContextName,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::AudioFormat,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::IsLossless,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::PlaybackOrigin,
            ))
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::plays::Column::Substituted,
            ))
            .into_model::<PlayRow>()
            .one(pool)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected database row"))?;
        assert_eq!(row.ns, "netease");
        assert_eq!(row.song_value, "42");
        assert_eq!(row.listen_ms, 3000);
        assert_eq!(row.finish_reason, FinishReason::Eof);
        assert_eq!(row.play_mode, "repeat_one");
        assert_eq!(row.session_id, sid);
        assert_eq!(row.origin_kind, PlayOrigin::Explicit);
        assert_eq!(row.actor, Actor::User);
        assert_eq!(row.context_kind, "playlist");
        assert_eq!(row.context_ref, Some("netease:7".to_owned()));
        assert_eq!(
            row.context_name,
            Some("收藏夹".to_owned()),
            "语境显示名快照"
        );
        assert_eq!(row.audio_format, Some("flac".to_owned()));
        assert_eq!(row.is_lossless, Some(1), "flac 无损 → 1");
        assert_eq!(row.playback_origin, PlaybackOrigin::Download);
        assert_eq!(row.substituted, 0);
        Ok(())
    }

    #[tokio::test]
    async fn record_play_disabled_is_noop() -> color_eyre::Result<()> {
        let store = StatsStore::disabled();
        store.record_play(&sample(1)).await?;
        Ok(())
    }

    #[tokio::test]
    async fn touch_session_updates_ended_at() -> color_eyre::Result<()> {
        let (_dir, store) = open_temp().await?;
        let sid = store
            .open_session(1000)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected session id"))?;
        store.touch_session(sid, 5000).await?;
        let pool = store
            .pool()
            .ok_or_else(|| color_eyre::eyre::eyre!("expected live pool"))?;
        let ended = crate::entity::sessions::Entity::find()
            .select_only()
            .expr(sea_orm::sea_query::Expr::col(
                crate::entity::sessions::Column::EndedAt,
            ))
            .filter(
                sea_orm::sea_query::Expr::col(crate::entity::sessions::Column::Id)
                    .eq(sea_orm::sea_query::Expr::value(sid)),
            )
            .into_tuple::<i64>()
            .one(pool)
            .await?
            .ok_or_else(|| color_eyre::eyre::eyre!("expected database row"))?;
        assert_eq!(ended, 5000);
        Ok(())
    }
}

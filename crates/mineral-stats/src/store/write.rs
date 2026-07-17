//! plays / sessions 事实行的写入。

use color_eyre::eyre::WrapErr as _;
use mineral_model::AudioFormat;

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
        let id = sqlx::query!(
            "INSERT INTO sessions (started_at, ended_at) VALUES (?, ?)",
            started_at,
            started_at
        )
        .execute(pool)
        .await
        .wrap_err("open_session 落库失败")?
        .last_insert_rowid();
        Ok(Some(id))
    }

    /// 随播放活动推进,更新会话结束时刻。降级时 no-op。
    pub async fn touch_session(&self, session_id: i64, ended_at: i64) -> color_eyre::Result<()> {
        let Some(pool) = self.pool() else {
            return Ok(());
        };
        sqlx::query!(
            "UPDATE sessions SET ended_at = ? WHERE id = ?",
            ended_at,
            session_id
        )
        .execute(pool)
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
        let audio_format = rec.audio.audio_format.as_ref().map(AudioFormat::as_str);
        let is_lossless = rec
            .audio
            .audio_format
            .as_ref()
            .map(|f| i64::from(f.is_lossless()));
        let quality = rec.audio.quality.map(|q| q.as_str());
        let substituted = i64::from(rec.audio.substituted);
        sqlx::query!(
            "INSERT INTO plays (
                ns, song_value, started_at, ended_at, listen_ms, duration_ms_snapshot,
                finish_reason, skip_at_ms, play_mode, session_id, origin_kind, actor,
                context_kind, context_ref, audio_format, is_lossless, bitrate_bps,
                quality, bit_depth, playback_origin, substituted
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            ns,
            song_value,
            rec.started_at,
            rec.ended_at,
            rec.listen_ms,
            rec.duration_ms_snapshot,
            rec.finish_reason as _,
            rec.skip_at_ms,
            rec.play_mode as _,
            rec.session_id,
            rec.origin as _,
            rec.actor as _,
            context_kind,
            context_ref,
            audio_format,
            is_lossless,
            rec.audio.bitrate_bps,
            quality,
            rec.audio.bit_depth,
            rec.playback_origin as _,
            substituted,
        )
        .execute(pool)
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

    /// 读回断言用的行结构(运行期 query_as + 类型化枚举 decode)。
    #[derive(sqlx::FromRow)]
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
        let row = sqlx::query_as::<_, PlayRow>(
            "SELECT ns, song_value, listen_ms, finish_reason, play_mode, session_id, \
             origin_kind, actor, context_kind, context_ref, audio_format, is_lossless, \
             playback_origin, substituted FROM plays",
        )
        .fetch_one(pool)
        .await?;
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
        let ended = sqlx::query_scalar::<_, i64>("SELECT ended_at FROM sessions WHERE id = ?")
            .bind(sid)
            .fetch_one(pool)
            .await?;
        assert_eq!(ended, 5000);
        Ok(())
    }
}

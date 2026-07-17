//! scheduler 任务事件的消化与分流。
//!
//! 每 tick 一次 drain:`PlayUrlReady` / `LyricsReady` 在 server 内部消化
//! (进 PlayerSync 的 current 重段,不转发);`PlaylistsFetched` 进歌单库
//! 聚合态(client 只见出口变换后的 LibrarySnapshot);`PlaylistWriteDone`
//! 成功时先触发缓存收敛再转发;其余原样进 client_events buffer 等 client 拉走。

use mineral_model::{PlayUrl, Song, SongId};
use mineral_task::{ChannelFetchKind, PlaylistWriteOp, Priority, TaskEvent, TaskKind, WriteError};

use crate::player::PlayerCore;

/// 跨进程写错误([`WriteError`])→ 埋点的失败归类([`mineral_stats::PlaylistError`])。
fn map_write_error(e: &WriteError) -> mineral_stats::PlaylistError {
    match e {
        WriteError::AuthRequired => mineral_stats::PlaylistError::AuthRequired,
        WriteError::RateLimited => mineral_stats::PlaylistError::RateLimited,
        WriteError::NotSupported => mineral_stats::PlaylistError::NotSupported,
        WriteError::Api { .. } => mineral_stats::PlaylistError::Api,
        WriteError::Other(_) => mineral_stats::PlaylistError::Other,
    }
}

impl PlayerCore {
    /// 一次 drain scheduler events,分类:PlayUrlReady/LyricsReady 内部消化、
    /// PlaylistWriteDone 成功先收敛缓存、全部业务事件 push 到 client_events buffer。
    pub(crate) fn consume_events_once(&self) {
        let events = self.inner.scheduler.drain_events();
        if events.is_empty() {
            return;
        }
        let mut forward = Vec::with_capacity(events.len());
        for ev in events {
            match ev {
                TaskEvent::PlayUrlReady { song_id, play_url } => {
                    self.handle_play_url_ready(&song_id, play_url);
                }
                TaskEvent::SongUrlFailed { song_id } => {
                    self.handle_song_url_failed(&song_id);
                }
                TaskEvent::LyricsReady { song_id, lyrics } => {
                    self.handle_lyrics_ready(&song_id, lyrics);
                }
                // 逐源列表进聚合态,client 只见出口变换后的合并快照
                // (LibrarySnapshot,由管线异步推)。
                TaskEvent::PlaylistsFetched { source, playlists } => {
                    self.library_concluded(source, Some(playlists));
                }
                TaskEvent::SearchResults {
                    source,
                    kind,
                    query,
                    page,
                    payload,
                    has_more,
                } => {
                    // source/kind/page 是 Copy,记录后原样转发给 client。
                    self.record_search_result(source, kind, &query, page, &payload);
                    forward.push(TaskEvent::SearchResults {
                        source,
                        kind,
                        query,
                        page,
                        payload,
                        has_more,
                    });
                }
                // 纯埋点信号:记 fetches 后**不转发**(client 不消费)。
                TaskEvent::FetchDone {
                    kind,
                    source,
                    target_ref,
                    from_user,
                    outcome,
                    latency_ms,
                } => {
                    self.record_fetch(kind, source, target_ref, from_user, outcome, latency_ms);
                }
                TaskEvent::PlaylistWriteDone { op, error } => {
                    self.record_playlist_op(&op, error.as_ref());
                    if error.is_none() {
                        self.refresh_after_write(&op);
                    }
                    forward.push(TaskEvent::PlaylistWriteDone { op, error });
                }
                other => forward.push(other),
            }
        }
        if !forward.is_empty() {
            self.inner.client_events.lock().extend(forward);
        }
    }

    /// 写成功后的缓存收敛:**不直接改数据,只提交重拉任务**——数据重建走
    /// "远端为事实源 + 版本戳"的现有读管线,写路径与读路径永远只有一套真相。
    /// (netease 写后 `trackUpdateTime` 必然变化,自动命中"版本变 → 全拉"分支。)
    fn refresh_after_write(&self, op: &PlaylistWriteOp) {
        match op {
            PlaylistWriteOp::AddSongs { id, .. } | PlaylistWriteOp::RemoveSongs { id, .. } => {
                self.inner.scheduler.submit(
                    TaskKind::ChannelFetch(ChannelFetchKind::PlaylistDetail { id: id.clone() }),
                    Priority::User,
                );
            }
            PlaylistWriteOp::Create { source, .. } => {
                self.submit_my_playlists(*source);
            }
            PlaylistWriteOp::Delete { id }
            | PlaylistWriteOp::Rename { id, .. }
            | PlaylistWriteOp::SetDescription { id, .. } => {
                self.submit_my_playlists(id.namespace());
            }
        }
    }

    /// PlayUrlReady 命中当前歌 → audio.play + 写 play_url;命中正在预拉的下一首 → gapless 预排;否则丢。
    pub(crate) fn handle_play_url_ready(&self, song_id: &SongId, play_url: PlayUrl) {
        // 先在锁内分类(三选一),放锁后再做会重新加锁的动作(play_capturing / gapless 预排)。
        enum Route {
            Current(Option<Box<Song>>),
            Prefetch,
            Drop,
        }
        let route = {
            let mut st = self.inner.state.lock();
            let want = st.current_song.as_ref().map(|t| &t.id);
            if want == Some(song_id) {
                st.play_url = Some(play_url.clone());
                st.bump_current();
                Route::Current(st.current_song.clone().map(Box::new))
            } else if st.prefetch_fired_for.as_ref() == Some(song_id) {
                Route::Prefetch
            } else {
                Route::Drop
            }
        };
        // 埋点:取链成功(url_resolutions;Drop=队列已变的陈旧结果,意图不明,不记)。
        match &route {
            Route::Current(_) => {
                self.record_url_resolution(song_id, mineral_stats::UrlOutcome::Ok, false);
                // 富化在播行的音频快照:此 URL 即当前起播曲的(Prefetch 的是下一曲、不改
                // 当前 pending;Drop 已作废)。
                self.enrich_from_play_url(&play_url);
            }
            Route::Prefetch => {
                self.record_url_resolution(song_id, mineral_stats::UrlOutcome::Ok, true);
            }
            Route::Drop => {}
        }
        match route {
            Route::Current(song) => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "play", "play url ready");
                if let Some(song) = song {
                    // 拦截桥:无脚本同步直走,有脚本异步裁决(play_url 已在锁内写过,
                    // 桥内回填同值幂等;改写时回填改写值)。
                    crate::hook_bridge::before_stream(self, &song, play_url);
                }
            }
            Route::Prefetch => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "prefetch", "play url ready");
                // 拦截桥(预取提交点):无脚本直走武装,有脚本异步裁决后再武装/否决。
                crate::hook_bridge::on_prefetch_ready(self, song_id, play_url);
            }
            Route::Drop => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "drop", "play url ready");
            }
        }
    }

    /// SongUrlFailed 命中当前歌 → 即时口 unplayable 拦截(无脚本维持
    /// `track_finished("error")` 的原失败语义);命中正在预拉的下一首 → 预取口
    /// unplayable 拦截(无脚本静默,边界 Fallback 兜底);否则丢。
    pub(crate) fn handle_song_url_failed(&self, song_id: &SongId) {
        enum Route {
            Current(Box<Song>),
            Prefetch,
            Drop,
        }
        let route = {
            let st = self.inner.state.lock();
            let want = st.current_song.as_ref().map(|t| &t.id);
            if want == Some(song_id) {
                match st.current_song.clone() {
                    Some(song) => Route::Current(Box::new(song)),
                    None => Route::Drop,
                }
            } else if st.prefetch_fired_for.as_ref() == Some(song_id) {
                Route::Prefetch
            } else {
                Route::Drop
            }
        };
        // 埋点:取链失败(url_resolutions;Drop=陈旧结果不记)。
        match &route {
            Route::Current(_) => {
                self.record_url_resolution(song_id, mineral_stats::UrlOutcome::Error, false);
            }
            Route::Prefetch => {
                self.record_url_resolution(song_id, mineral_stats::UrlOutcome::Error, true);
            }
            Route::Drop => {}
        }
        match route {
            Route::Current(song) => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "unplayable", "song url failed");
                crate::hook_bridge::on_unplayable_current(self, &song);
            }
            Route::Prefetch => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "prefetch_unplayable", "song url failed");
                crate::hook_bridge::on_unplayable_prefetch(self, song_id);
            }
            Route::Drop => {
                mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "drop", "song url failed");
            }
        }
    }

    /// LyricsReady 命中当前歌 → 写入 current_lyrics + 配对 song_id;否则丢(只缓存当前歌)。
    pub(crate) fn handle_lyrics_ready(&self, song_id: &SongId, lyrics: mineral_model::Lyrics) {
        let mut st = self.inner.state.lock();
        let want = st.current_song.as_ref().map(|t| &t.id);
        if want == Some(song_id) {
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "store", "lyrics ready");
            st.current_lyrics = Some(lyrics);
            st.current_lyrics_song_id = Some(song_id.clone());
            st.bump_current();
        } else {
            // 非当前歌,无意义,丢(只缓存当前歌)。
            mineral_log::debug!(target: "player", song_id = song_id.as_str(), action = "drop", "lyrics ready");
        }
    }

    /// `TaskEvent::FetchDone` → fetches 埋点(行为域)。actor 随 trigger:user 触发归
    /// User、系统链路(预取 / 回填)归 System;outcome / latency 直落。
    ///
    /// # Params:
    ///   - `kind`: 取数种类
    ///   - `source`: 来源
    ///   - `target_ref`: 目标 qualified 引用(无目标为 `None`)
    ///   - `from_user`: 是否 user 优先级触发
    ///   - `outcome`: 收束结局
    ///   - `latency_ms`: 耗时 ms
    pub(crate) fn record_fetch(
        &self,
        kind: mineral_task::ChannelFetchKindTag,
        source: mineral_model::SourceKind,
        target_ref: Option<String>,
        from_user: bool,
        outcome: mineral_task::TaskOutcome,
        latency_ms: u64,
    ) {
        // task 层取数种类 → 埋点词汇(穷尽:task 加变体必须在此表态归属)。
        let fetch_kind = match kind {
            mineral_task::ChannelFetchKindTag::MyPlaylists => mineral_stats::FetchKind::MyPlaylists,
            mineral_task::ChannelFetchKindTag::PlaylistDetail => {
                mineral_stats::FetchKind::PlaylistDetail
            }
            mineral_task::ChannelFetchKindTag::SongUrl => mineral_stats::FetchKind::SongUrl,
            mineral_task::ChannelFetchKindTag::Lyrics => mineral_stats::FetchKind::Lyrics,
            mineral_task::ChannelFetchKindTag::RemotePlayCount => {
                mineral_stats::FetchKind::RemotePlayCount
            }
            mineral_task::ChannelFetchKindTag::Search => mineral_stats::FetchKind::Search,
            mineral_task::ChannelFetchKindTag::ArtistDetail => {
                mineral_stats::FetchKind::ArtistDetail
            }
            mineral_task::ChannelFetchKindTag::ArtistAlbums => {
                mineral_stats::FetchKind::ArtistAlbums
            }
            mineral_task::ChannelFetchKindTag::AlbumDetail => mineral_stats::FetchKind::AlbumDetail,
        };
        let (actor, trigger) = if from_user {
            (
                mineral_stats::Actor::User,
                mineral_stats::FetchTrigger::User,
            )
        } else {
            (
                mineral_stats::Actor::System,
                mineral_stats::FetchTrigger::System,
            )
        };
        let fetch_outcome = match outcome {
            mineral_task::TaskOutcome::Ok => mineral_stats::FetchOutcome::Ok,
            mineral_task::TaskOutcome::Failed => mineral_stats::FetchOutcome::Failed,
            mineral_task::TaskOutcome::Cancelled => mineral_stats::FetchOutcome::Cancelled,
        };
        self.inner.stats.event(mineral_stats::StatsEvent::Behavior {
            actor,
            event: mineral_stats::BehaviorEvent::Fetch {
                fetch_kind,
                source,
                target_ref,
                trigger,
                outcome: fetch_outcome,
                latency_ms: i64::try_from(latency_ms).unwrap_or(i64::MAX),
            },
        });
    }

    /// `TaskEvent::SearchResults` → searches 埋点(actor=User——界面搜索)。
    /// `SearchKind::User` 不记(埋点只覆盖四类实体搜索);只在成功侧记(失败任务不发此事件)。
    ///
    /// # Params:
    ///   - `source`: 搜索来源
    ///   - `kind`: 搜索实体类型
    ///   - `query`: 搜索词原文(记录侧据 search_queries 档决定原文 / 散列 / 不记)
    ///   - `page`: 分页参数(页码 = offset / limit)
    ///   - `payload`: 结果载荷(取条数)
    pub(crate) fn record_search_result(
        &self,
        source: mineral_model::SourceKind,
        kind: mineral_model::SearchKind,
        query: &str,
        page: mineral_channel_core::Page,
        payload: &mineral_task::SearchPayload,
    ) {
        use mineral_model::SearchKind;
        use mineral_task::SearchPayload;
        let target = match kind {
            SearchKind::Song => mineral_stats::SearchTargetKind::Song,
            SearchKind::Album => mineral_stats::SearchTargetKind::Album,
            SearchKind::Artist => mineral_stats::SearchTargetKind::Artist,
            SearchKind::Playlist => mineral_stats::SearchTargetKind::Playlist,
            SearchKind::User => return, // 埋点不记用户搜索
        };
        let count = match payload {
            SearchPayload::Songs(v) => v.len(),
            SearchPayload::Albums(v) => v.len(),
            SearchPayload::Playlists(v) => v.len(),
            SearchPayload::Artists(v) => v.len(),
        };
        let page_no = i64::from(page.offset.checked_div(page.limit).unwrap_or(0));
        self.inner.stats.record_search(
            mineral_stats::Actor::User,
            query,
            target,
            source,
            page_no,
            Some(i64::try_from(count).unwrap_or(i64::MAX)),
            mineral_stats::SearchOutcome::Ok,
        );
    }

    /// 记一次歌单写结局(playlist_ops;行为域,actor=User——歌单编辑是用户库管理动作)。
    ///
    /// # Params:
    ///   - `op`: 写操作(定 op 名 / 歌单 ref / 涉及单曲)
    ///   - `error`: 失败错误;`None` 为成功
    pub(crate) fn record_playlist_op(
        &self,
        op: &PlaylistWriteOp,
        error: Option<&mineral_task::WriteError>,
    ) {
        use mineral_stats::{PlaylistOpKind, PlaylistRef};
        let (op_name, playlist_ref) = match op {
            PlaylistWriteOp::Create { source, name } => (
                PlaylistOpKind::Create,
                PlaylistRef::Creating {
                    source: *source,
                    name: name.clone(),
                },
            ),
            PlaylistWriteOp::Delete { id } => {
                (PlaylistOpKind::Delete, PlaylistRef::Existing(id.clone()))
            }
            PlaylistWriteOp::AddSongs { id, .. } => {
                (PlaylistOpKind::Add, PlaylistRef::Existing(id.clone()))
            }
            PlaylistWriteOp::RemoveSongs { id, .. } => {
                (PlaylistOpKind::Remove, PlaylistRef::Existing(id.clone()))
            }
            PlaylistWriteOp::Rename { id, .. } => {
                (PlaylistOpKind::Rename, PlaylistRef::Existing(id.clone()))
            }
            PlaylistWriteOp::SetDescription { id, .. } => (
                PlaylistOpKind::SetDescription,
                PlaylistRef::Existing(id.clone()),
            ),
        };
        let songs = op.songs();
        // 恰一首才落 song 列(多首为整批操作,单列表达不了,置 None);count 记全量。
        let song = if songs.len() == 1 {
            songs.first().cloned()
        } else {
            None
        };
        let (outcome, error_kind) = match error {
            None => (mineral_stats::OpOutcome::Ok, None),
            Some(e) => (mineral_stats::OpOutcome::Failed, Some(map_write_error(e))),
        };
        self.inner.stats.event(mineral_stats::StatsEvent::Behavior {
            actor: mineral_stats::Actor::User,
            event: mineral_stats::BehaviorEvent::PlaylistOp {
                op: op_name,
                playlist_ref,
                song,
                song_count: i64::try_from(songs.len()).unwrap_or(i64::MAX),
                outcome,
                error_kind,
            },
        });
    }

    /// 记一次取播放链结局(url_resolutions;系统域,无 actor)。
    ///
    /// # Params:
    ///   - `song`: 取链的歌曲
    ///   - `outcome`: 结局(拿到 / 空 / 报错)
    ///   - `for_prefetch`: 是否为预取取链(非当前起播)
    fn record_url_resolution(
        &self,
        song: &SongId,
        outcome: mineral_stats::UrlOutcome,
        for_prefetch: bool,
    ) {
        self.inner.stats.event(mineral_stats::StatsEvent::System(
            mineral_stats::SystemEvent::UrlResolution {
                song: song.clone(),
                quality_requested: self.playback_quality().as_str().to_owned(),
                outcome,
                for_prefetch,
            },
        ));
    }
}

//! 服务端持有的「播放上下文」内部状态。[`crate::player::PlayerCore`] 用 `Mutex<State>` 包它,
//! 队列计算([`crate::queue`])与播放模式切换直接读写其字段。

use mineral_model::{Envelope, PlayUrl, Song, SongId};
use mineral_protocol::{
    CurrentSync, PlayCursor, PlayMode, PlaybackOrigin, PlayerSync, PlayerVersions, QueueSync,
};

use crate::download::Capturing;

/// 一次队列编辑前的队列全貌,供撤销还原。
///
/// 只回滚队列结构,**不回滚播放**:撤销一次「删在播曲」后,那首歌回到队列里、游标从悬空
/// 变回附着,但它自始至终没有断过声。
pub(crate) struct QueueSnapshot {
    /// 编辑前的队列。
    pub(crate) queue: Vec<Song>,

    /// 编辑前的 shuffle 原序。
    pub(crate) original_queue: Option<Vec<Song>>,

    /// 编辑前的游标。
    pub(crate) cursor: PlayCursor,
}

/// 播放上下文。字段对 crate 内开放(队列计算 / 模式切换 / 播放编排直接读写)。
pub(crate) struct State {
    /// 当前在播的歌。
    pub(crate) current_song: Option<Song>,

    /// 当前歌的播放 URL(从 SongUrlReady 写入)。
    pub(crate) play_url: Option<PlayUrl>,

    /// 当前在播音频的来源(下载 / 缓存 / 远端);切歌时由 `play_song` 写入。
    pub(crate) play_origin: Option<PlaybackOrigin>,

    /// 当前队列的语境(来自搜索 / 歌单 / 专辑 / 艺人 / 手动;`set_queue` 时写入)。
    /// 埋点 provenance:随该队列每个起播继承进 plays 的 context 列。
    pub(crate) queue_context: mineral_stats::QueueContext,

    /// 插队散曲的 per-song 语境覆盖(`qualified id → QueueContext`;insert_next / append
    /// 时按曲落一条)。起播该曲时优先取覆盖值再移除,使插队曲记自身来源而非继承队列级
    /// context——不污染歌单归属。
    pub(crate) context_overrides: rustc_hash::FxHashMap<String, mineral_stats::QueueContext>,

    /// 当前队列(顺序模式 = 原序;shuffle 模式 = 洗过)。
    pub(crate) queue: Vec<Song>,

    /// 当前歌在 `queue` 中的位置(悬空态见 [`PlayCursor::Detached`])。
    pub(crate) cursor: PlayCursor,

    /// shuffle 切换前的原始顺序,关 shuffle 时还原用;非 shuffle 模式下为 `None`。
    pub(crate) original_queue: Option<Vec<Song>>,

    /// 上一次队列编辑前的快照,供撤销。单级:撤销后即清空,不支持连撤与重做。
    pub(crate) queue_undo: Option<QueueSnapshot>,

    /// 当前播放模式(顺序 / 单曲 / 列表循环 / shuffle)。
    pub(crate) play_mode: PlayMode,

    /// 当前歌的歌词(从 LyricsReady 写入)。
    pub(crate) current_lyrics: Option<mineral_model::Lyrics>,

    /// 当前 lyrics 配对的歌 id(对不上 current_song 时不返回)。
    pub(crate) current_lyrics_song_id: Option<SongId>,

    /// 当前歌的振幅包络(id + 数据),离线算出 / db 命中后由 `adopt_envelope` 落此并 bump
    /// `current` 版本;`sync` 组段时按当前曲过滤,故串曲的迟到包络天然不外发。
    pub(crate) current_envelope: Option<(SongId, Envelope)>,

    /// 正在预拉(已发起 SongUrl 任务、URL 尚未回来)的下一曲 id;URL 到达时据此认领。
    /// 切歌 / 采纳后复位,避免对同一 next 重复预拉。
    pub(crate) prefetch_fired_for: Option<SongId>,

    /// 当前正在 capture(边播边落盘)的曲;自然播完 → 入缓存,中途打断 → 删残件。
    /// 命中缓存直接本地播时为 `None`(无需 capture)。
    pub(crate) capturing: Option<Capturing>,

    /// 已预排进 rodio 队列、等当前曲播完无缝接续的下一曲及其记账(gapless)。
    pub(crate) queued: Option<crate::gapless::Queued>,

    /// 本预取窗口内被 hook 否决(`Skip`)的队列**下标**:`next_index` 预测/推进时越过,
    /// 队列本身不动。按下标记(与推进「以下标为真相」同条,重复曲互不吸附);任何队列
    /// 变更/切歌/边界消费都清空,故下标不会陈旧。生命周期整族挂靠预取簿记
    /// (见 [`Self::invalidate_prefetch`])。
    pub(crate) prefetch_vetoed: Vec<usize>,

    /// queue + original_queue 的版本号。从 1 起步(0 = client 一无所有),变更处
    /// 经 [`Self::bump_queue`] 推进;[`Self::sync`] 据此决定是否附带 queue 重段。
    pub(crate) queue_version: u64,

    /// current_song / play_url / lyrics 的版本号,语义同 `queue_version`。
    pub(crate) current_version: u64,
}

impl State {
    /// 空 State,所有字段取默认/空值。
    pub(crate) fn empty() -> Self {
        Self {
            current_song: None,
            play_url: None,
            play_origin: None,
            queue_context: mineral_stats::QueueContext::Unknown,
            context_overrides: rustc_hash::FxHashMap::default(),
            queue: Vec::new(),
            cursor: PlayCursor::default(),
            queue_undo: None,
            original_queue: None,
            play_mode: PlayMode::default(),
            current_lyrics: None,
            current_lyrics_song_id: None,
            current_envelope: None,
            prefetch_fired_for: None,
            capturing: None,
            queued: None,
            prefetch_vetoed: Vec::new(),
            queue_version: 1,
            current_version: 1,
        }
    }

    /// 作废 gapless 预取簿记三件套(预排曲 / 预拉标记 / 否决集),让 `check_prefetch`
    /// 在下个 tick 按当前队列重排。队列变更 / 插播 / 追加 / 切歌等改变「下一首」预测
    /// 的地方调用;引擎里可能已建的 next 槽由调用方另行 `audio.clear_next()`。
    pub(crate) fn invalidate_prefetch(&mut self) {
        self.queued = None;
        self.prefetch_fired_for = None;
        self.prefetch_vetoed.clear();
    }

    /// 队列结构编辑后的预取维护——**精确**作废,不照抄 [`Self::invalidate_prefetch`]。
    ///
    /// 否决集存的是下标,重排后必然陈旧,无条件清。但已排的下一曲只在「下一首真的换了」
    /// 时才作废:删在播曲这类编辑并不改变下一首,连带作废反而打断 gapless——而且已 append
    /// 进 sink 的预排撤不掉,会先响半秒过期曲再被切走。
    ///
    /// # Return:
    ///   预排是否被作废(为真时调用方需同步取消引擎侧待建预排)。
    pub(crate) fn revalidate_prefetch_after_edit(&mut self) -> bool {
        self.prefetch_vetoed.clear();
        let armed = self
            .queued
            .as_ref()
            .map(|q| q.song.id.clone())
            .or_else(|| self.prefetch_fired_for.clone());
        let Some(armed) = armed else {
            return false;
        };
        if crate::queue::next_in_queue(self).is_some_and(|next| next.id == armed) {
            return false;
        }
        self.queued = None;
        self.prefetch_fired_for = None;
        true
    }

    /// 版本门控同步:轻段恒出,重段仅在 `known` 落后于本端版本时 clone 附带。
    ///
    /// 与版本号读取在同一把锁内(caller 持锁调用),无「版本与数据错位」竞态。
    ///
    /// # Params:
    ///   - `known`: client 已持有的版本号(0 = 一无所有)
    ///
    /// # Return:
    ///   组装好的 [`PlayerSync`]。
    pub(crate) fn sync(&self, known: PlayerVersions) -> PlayerSync {
        let queue = (known.queue != self.queue_version).then(|| QueueSync {
            queue: self.queue.clone(),
            original_queue: self.original_queue.clone(),
        });
        let current = (known.current != self.current_version).then(|| CurrentSync {
            current_song: self.current_song.clone(),
            play_url: self.play_url.clone(),
            current_lyrics: self.current_lyrics.clone(),
            current_lyrics_song_id: self.current_lyrics_song_id.clone(),
            // 只带归属当前曲的包络:预排下一曲 / 迟到旧曲的包络虽存在 slot 里,
            // 也在这里被过滤掉,不会串到别的曲上。
            current_envelope: self
                .current_envelope
                .as_ref()
                .filter(|(id, _)| self.current_song.as_ref().is_some_and(|s| s.id == *id))
                .map(|(_, envelope)| envelope.clone()),
        });
        PlayerSync {
            versions: PlayerVersions {
                queue: self.queue_version,
                current: self.current_version,
            },
            cursor: self.cursor,
            play_mode: self.play_mode,
            play_origin: self.play_origin,
            queue,
            current,
        }
    }

    /// queue / original_queue 发生变更后调用,推进版本号让 client 下次同步收到重段。
    pub(crate) fn bump_queue(&mut self) {
        self.queue_version += 1;
    }

    /// current_song / play_url / lyrics 发生变更后调用,推进版本号。
    pub(crate) fn bump_current(&mut self) {
        self.current_version += 1;
    }

    /// 收下一份算好 / db 命中的包络:仅当它归属**当前曲**才落 slot 并 bump `current`
    /// 版本(下次 `sync` 即随 `CurrentSync` 携带它重发)。非当前曲(如预排下一曲)的
    /// 包络此处忽略——它已落 db,待其成为当前曲时经 replay 载入,避免覆盖当前曲的 slot。
    pub(crate) fn adopt_envelope(&mut self, song_id: SongId, envelope: Envelope) {
        if self.current_song.as_ref().is_some_and(|s| s.id == song_id) {
            self.current_envelope = Some((song_id, envelope));
            self.bump_current();
        }
    }
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use mineral_protocol::{PlayCursor, PlayMode, PlayerVersions};
    use mineral_test::song;
    use pretty_assertions::assert_eq;

    use super::State;

    /// 造一个有 queue + current 的 State(版本仍是初始 1/1)。
    fn populated() -> State {
        let mut st = State::empty();
        st.queue = vec![song("a"), song("b")];
        st.cursor = PlayCursor::InQueue(1);
        st.current_song = Some(song("b"));
        st.play_mode = PlayMode::RepeatAll;
        st
    }

    /// known=0(client 一无所有)必然落后 → 两重段全量返回,轻段字段同出。
    #[test]
    fn sync_zero_versions_gets_full_payload() -> color_eyre::Result<()> {
        let st = populated();
        let sync = st.sync(PlayerVersions::default());
        assert_eq!(sync.versions.queue, 1);
        assert_eq!(sync.versions.current, 1);
        assert_eq!(sync.cursor, PlayCursor::InQueue(1));
        assert_eq!(sync.play_mode, PlayMode::RepeatAll);
        let q = sync.queue.ok_or_else(|| eyre!("queue 重段应存在"))?;
        assert_eq!(q.queue.len(), 2);
        assert!(q.original_queue.is_none());
        let c = sync.current.ok_or_else(|| eyre!("current 重段应存在"))?;
        assert_eq!(c.current_song, Some(song("b")));
        Ok(())
    }

    /// 版本一致 → 两重段都缺席(稳态 tick 的主路径,payload 仅轻段)。
    #[test]
    fn sync_matching_versions_light_only() {
        let st = populated();
        let sync = st.sync(PlayerVersions {
            queue: 1,
            current: 1,
        });
        assert!(sync.queue.is_none());
        assert!(sync.current.is_none());
        assert_eq!(sync.cursor, PlayCursor::InQueue(1));
        assert_eq!(sync.play_mode, PlayMode::RepeatAll);
    }

    /// 仅 queue 版本落后 → 只发 queue 重段,current 缺席。
    #[test]
    fn sync_stale_queue_only_sends_queue_section() {
        let mut st = populated();
        st.bump_queue();
        let sync = st.sync(PlayerVersions {
            queue: 1,
            current: 1,
        });
        assert_eq!(sync.versions.queue, 2);
        assert!(sync.queue.is_some());
        assert!(sync.current.is_none());
    }

    /// 仅 current 版本落后 → 只发 current 重段,queue 缺席。
    #[test]
    fn sync_stale_current_only_sends_current_section() {
        let mut st = populated();
        st.bump_current();
        let sync = st.sync(PlayerVersions {
            queue: 1,
            current: 1,
        });
        assert_eq!(sync.versions.current, 2);
        assert!(sync.queue.is_none());
        assert!(sync.current.is_some());
    }
}

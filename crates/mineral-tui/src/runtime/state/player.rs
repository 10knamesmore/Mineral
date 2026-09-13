//! server 权威播放态的 client 端镜像:在播歌、播放队列、洗牌备份、同步版本号。
//!
//! 每 tick 由 `apply_player_sync` 按版本门控灌入(重段缺席 = 与已有一致、原地保持)。
//! 区别于 [`Playback`](crate::runtime::playback::Playback):后者是本地播放状态机
//! (playing/paused/position/volume,由 audio snapshot 驱动);本域是「队列 + 在播 +
//! 同步簿记」,由 server 的 `PlayerSync` 驱动。

use mineral_model::Song;

/// server 播放态镜像([`AppState`](crate::runtime::state::AppState) 的同步域)。
pub struct PlayerMirror {
    /// server 同步的当前在播曲；没有在播曲时为 `None`。
    pub current: Option<Song>,

    /// 当前曲的进入档位(随 current 段到达;`None` = 尚未收到 / 无在播曲)。全屏切歌转场
    /// 据此定向:`Prev` 取反向动效,其余(含 `RandomAccess`)都按下一首。
    pub current_advance: Option<mineral_protocol::AdvanceKind>,

    /// 浮动 queue 当前曲目列表(后端权威态)。
    pub queue: Vec<Song>,

    /// server 的「在播位置锚点」:当前在播歌在 `queue` 中的位置(prev/next 由它推进)。
    /// 渲染 queue 浮层时按此下标为在播行着色并加下划线；队列含重复曲时身份匹配会
    /// 把全部副本一起点亮,只有下标能精确指出真正在播的那一行。
    ///
    /// 悬空态(在播曲已被摘出队列但仍在响)下没有任何一行该被标记。
    pub cursor: mineral_protocol::PlayCursor,

    /// Shuffle 状态下保存的原始 queue 顺序。退 Shuffle 时还原。
    /// 非 Shuffle 状态恒为 `None`。
    pub original_queue: Option<Vec<Song>>,

    /// 上次已应用的 server 状态版本号(每 tick 随 PlayerSync 回报;0 = 还没同步过,
    /// 首次同步必然全量)。
    pub versions: mineral_protocol::PlayerVersions,
}

impl PlayerMirror {
    /// 构造空镜像(无在播、空队列、版本归零)。
    pub(crate) fn new() -> Self {
        Self {
            current: None,
            current_advance: None,
            queue: Vec::new(),
            cursor: mineral_protocol::PlayCursor::default(),
            original_queue: None,
            versions: mineral_protocol::PlayerVersions::default(),
        }
    }
}

impl super::AppState {
    /// 当前在播歌在 queue 中的下标(打开浮层定位光标 / prefetch 邻居 / 封面预热都用它)。
    /// 无在播曲返回 `None`。
    ///
    /// 优先信任 server 的在播锚点——队列含重复曲时,这是唯一能精确指出在播是哪一行的
    /// 依据(按歌曲身份 first-match 会错指到首个副本)。仅当锚点确实指向在播歌时采纳;
    /// 否则(在播歌不在队列 / 锚点陈旧)退回身份 first-match,保住「在播曲不在队列」时
    /// 返回 `None` 的既有语义。
    ///
    /// 锚点悬空(在播曲已被摘出队列但仍在响)时直接返回 `None` 且**不**回落身份匹配:
    /// 队列里若还留着同一首歌的另一份,回落会把那一行错点亮成「正在播」。
    pub fn queue_current_index(&self) -> Option<usize> {
        let id = &self.playback.track.as_ref()?.id;
        let sel = self.player.cursor.queue_index()?;
        if self.player.queue.get(sel).is_some_and(|s| &s.id == id) {
            return Some(sel);
        }
        self.player.queue.iter().position(|s| &s.id == id)
    }

    /// 在播曲前后各 `range` 首在 `queue` 中的下标,按播放模式的环回语义算——切歌落点
    /// (`n` / `p` / 自动接续)走的就是这些位置,封面预热必须跟上,否则环回那一端永远冷。
    /// Sequential 到两端即止;其余模式(`RepeatAll` / `Shuffle` / `RepeatOne`)环回。
    ///
    /// # Params:
    ///   - `range`: 前后各看几首
    ///
    /// # Return:
    ///   邻居下标(前一 / 后一交替,去重);无在播曲 / 空队列返回空。
    pub fn queue_neighbor_indexes(&self, range: usize) -> Vec<usize> {
        let len = self.player.queue.len();
        let Some(pos) = self.queue_current_index() else {
            return Vec::new();
        };
        if range == 0 {
            return Vec::new();
        }
        let wraps = self.playback.mode != mineral_protocol::PlayMode::Sequential;
        let mut out = Vec::with_capacity(range.saturating_mul(2));
        for d in 1..=range {
            let neighbors = if wraps {
                let step = d % len;
                [Some((pos + len - step) % len), Some((pos + step) % len)]
            } else {
                [pos.checked_sub(d), (pos + d < len).then_some(pos + d)]
            };
            for idx in neighbors.into_iter().flatten() {
                if !out.contains(&idx) {
                    out.push(idx);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::super::AppState;
    use crate::test_support::endserenading;

    /// 邻居按播放模式环回:Shuffle 下 pos=0 的「上一首」是队尾(daemon 的 `prev_index`
    /// 就是 `(cur + len - 1) % len`),Sequential 则到两端为止。
    #[test]
    fn queue_neighbor_indexes_follow_play_mode_wrap() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        let queue = endserenading(5);
        s.playback.track = queue.first().cloned();
        s.player.queue = queue;

        s.playback.mode = mineral_protocol::PlayMode::Shuffle;
        assert_eq!(
            s.queue_neighbor_indexes(1),
            vec![4, 1],
            "队首的上一首应环回队尾"
        );
        assert_eq!(s.queue_neighbor_indexes(2), vec![4, 1, 3, 2]);

        s.playback.mode = mineral_protocol::PlayMode::Sequential;
        assert_eq!(
            s.queue_neighbor_indexes(1),
            vec![1],
            "顺序模式队首没有上一首"
        );

        s.playback.track = s.player.queue.get(4).cloned();
        assert_eq!(
            s.queue_neighbor_indexes(1),
            vec![3],
            "顺序模式队尾没有下一首"
        );
        Ok(())
    }

    /// `queue_current_index` 命中在播歌下标;无在播曲返回 `None`。
    #[test]
    fn queue_current_index_finds_playing() -> color_eyre::Result<()> {
        let mut s = AppState::test_default()?;
        let queue = endserenading(5);
        s.playback.track = queue.get(2).cloned();
        s.player.queue = queue;
        assert_eq!(s.queue_current_index(), Some(2));

        s.playback.track = None;
        assert_eq!(s.queue_current_index(), None);
        Ok(())
    }

    /// 重复曲:`queue_current_index` 采纳 server 锚点,精确指向在播的那个
    /// 副本,而非按身份回退到首个副本。
    #[test]
    fn queue_current_index_prefers_anchor_over_identity() -> color_eyre::Result<()> {
        use mineral_test::song;
        let mut s = AppState::test_default()?;
        s.player.queue = vec![song("a"), song("b"), song("a"), song("b")];
        s.playback.track = Some(song("a"));
        s.player.cursor = mineral_protocol::PlayCursor::InQueue(2); // 第二个 a 正在播
        assert_eq!(s.queue_current_index(), Some(2), "应采纳锚点,而非首个 a@0");

        // 锚点不指向在播歌(在播曲不在队列)→ 退回身份匹配,找不到返回 None。
        s.playback.track = Some(song("z"));
        s.player.cursor = mineral_protocol::PlayCursor::InQueue(2);
        assert_eq!(s.queue_current_index(), None, "在播曲不在队列时仍返回 None");
        Ok(())
    }
}

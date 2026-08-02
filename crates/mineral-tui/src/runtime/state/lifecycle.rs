//! 把播放生命周期事件收敛进依赖其失效语义的 TUI cache。

use mineral_model::SongId;
use mineral_protocol::FinishReason;

use super::AppState;

impl AppState {
    /// 应用一条曲目结束事件，维护本地自然播完次数 cache。
    ///
    /// 只有 eof 会改变完整播放次数。已有 cache 直接加一；查询仍在飞行时将其 response
    /// 标为过期，避免旧值覆盖刚发生的完播事实。
    ///
    /// # Params:
    ///   - `song_id`: 结束的歌曲
    ///   - `reason`: 曲目结束原因
    pub(crate) fn apply_track_finished(&mut self, song_id: &SongId, reason: FinishReason) {
        if reason != FinishReason::Eof || !self.records_local_plays_for(song_id.namespace()) {
            return;
        }
        if self.library.local_play_counts.note_eof(song_id) {
            self.redecorate_for_source(song_id.namespace());
        }
    }
}

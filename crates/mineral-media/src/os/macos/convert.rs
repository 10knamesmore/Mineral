//! 平台无关类型 ↔ MediaPlayer 框架类型的纯映射(无 FFI,可独立单测)。

use std::time::Duration;

use objc2_media_player::{MPNowPlayingPlaybackState, MPRepeatType, MPShuffleType};

use crate::command::LoopMode;
use crate::state::PlaybackState;

/// [`PlaybackState`] → 系统 Now Playing 播放态。
pub(super) fn to_now_playing_state(state: PlaybackState) -> MPNowPlayingPlaybackState {
    match state {
        PlaybackState::Playing => MPNowPlayingPlaybackState::Playing,
        PlaybackState::Paused => MPNowPlayingPlaybackState::Paused,
        PlaybackState::Stopped => MPNowPlayingPlaybackState::Stopped,
    }
}

/// 播放态 → `MPNowPlayingInfoPropertyPlaybackRate`:播放为 1.0,其余为 0.0。
///
/// 速率配合 elapsed 让系统进度条在两次上报之间自行外推前进。
pub(super) fn playback_rate(state: PlaybackState) -> f64 {
    match state {
        PlaybackState::Playing => 1.0,
        PlaybackState::Paused | PlaybackState::Stopped => 0.0,
    }
}

/// `Duration` → 秒(f64);MediaPlayer 的时长 / 进度都以秒计。
pub(super) fn secs(d: Duration) -> f64 {
    d.as_secs_f64()
}

/// 系统 `MPRepeatType`(用户在控件改的循环模式)→ 平台无关 [`LoopMode`]。
pub(super) fn repeat_to_loop(repeat: MPRepeatType) -> LoopMode {
    match repeat {
        MPRepeatType::One => LoopMode::Track,
        MPRepeatType::All => LoopMode::Playlist,
        // Off 及未知值都按「不循环」。
        _ => LoopMode::None,
    }
}

/// 系统 `MPShuffleType`(用户在控件改的随机模式)→ 是否随机。
pub(super) fn shuffle_to_bool(shuffle: MPShuffleType) -> bool {
    // Off=不随机;Items / Collections 都视为随机开。
    shuffle != MPShuffleType::Off
}

#[cfg(test)]
mod tests {
    use super::{playback_rate, repeat_to_loop, secs, shuffle_to_bool, to_now_playing_state};
    use crate::command::LoopMode;
    use crate::state::PlaybackState;
    use objc2_media_player::{MPNowPlayingPlaybackState, MPRepeatType, MPShuffleType};
    use std::time::Duration;

    #[test]
    fn playback_state_maps_to_now_playing_state() {
        assert_eq!(
            to_now_playing_state(PlaybackState::Playing),
            MPNowPlayingPlaybackState::Playing
        );
        assert_eq!(
            to_now_playing_state(PlaybackState::Paused),
            MPNowPlayingPlaybackState::Paused
        );
        assert_eq!(
            to_now_playing_state(PlaybackState::Stopped),
            MPNowPlayingPlaybackState::Stopped
        );
    }

    #[test]
    fn rate_is_one_only_when_playing() {
        assert_eq!(playback_rate(PlaybackState::Playing), 1.0);
        assert_eq!(playback_rate(PlaybackState::Paused), 0.0);
        assert_eq!(playback_rate(PlaybackState::Stopped), 0.0);
    }

    #[test]
    fn duration_to_secs() {
        assert_eq!(secs(Duration::from_millis(1500)), 1.5);
        assert_eq!(secs(Duration::ZERO), 0.0);
    }

    #[test]
    fn repeat_type_maps_to_loop_mode() {
        assert_eq!(repeat_to_loop(MPRepeatType::Off), LoopMode::None);
        assert_eq!(repeat_to_loop(MPRepeatType::One), LoopMode::Track);
        assert_eq!(repeat_to_loop(MPRepeatType::All), LoopMode::Playlist);
    }

    #[test]
    fn shuffle_type_maps_to_bool() {
        assert!(!shuffle_to_bool(MPShuffleType::Off));
        assert!(shuffle_to_bool(MPShuffleType::Items));
        assert!(shuffle_to_bool(MPShuffleType::Collections));
    }
}

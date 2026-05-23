//! 系统媒体服务(MPRIS)接入:把 [`PlayerCore`] 的播放状态上报给系统媒体控件,
//! 把控件发来的命令转成播放控制。
//!
//! 只在 daemon(常驻 server)里启用 —— 系统媒体控件控制的是常驻播放,关掉 TUI
//! 仍然有效。in-proc 模式(TUI 自起 server)不调用本模块,避免多实例抢同一条
//! D-Bus 总线名。

use std::sync::Arc;
use std::time::{Duration, Instant};

use mineral_audio::AudioSnapshot;
use mineral_media::{MediaCommand, MediaConfig, MediaService, NowPlaying, PlaybackState};
use mineral_model::{Lyrics, MediaUrl, Song, SongId};
use mineral_protocol::PlayerSnapshot;

use crate::player::PlayerCore;

/// 状态上报循环的间隔。200ms 让 Position 跟手,显示端按它做逐行歌词同步才平滑
const REPORT_INTERVAL_MS: u64 = 200;

/// 判定 seek 跳变的阈值(ms)。实际位置偏离「线性外推预期」超过它就当作 seek。
/// 取 1s:远大于 tick 抖动 / 采样误差,又远小于任何真实 seek 步长(最小 5s)。
const SEEK_THRESHOLD_MS: u64 = 1000;

/// 起系统媒体服务:注册控件、attach 命令回调、spawn 状态上报 task。
///
/// # Params:
///   - `player`: 服务端播放核心(命令回调与上报都打到它)。
///
/// # Return:
///   注册失败(如无 D-Bus session)返回 `Err`;调用方可据此降级(daemon 照常跑)。
pub(crate) fn start(player: PlayerCore) -> color_eyre::Result<()> {
    let cmd_player = player.clone();
    let on_command: Arc<dyn Fn(MediaCommand) + Send + Sync> =
        Arc::new(move |cmd| handle_command(&cmd_player, cmd));
    // identity 与 bus 后缀一致(都 "mineral"):显示端常用 `playerctl -p <identity>`
    // 拉数据,而 playerctl 的 -p 匹配的是 bus 后缀,大小写需一致。
    let config = MediaConfig::builder()
        .dbus_name("mineral")
        .display_name("mineral")
        .build();
    let service = MediaService::spawn(&config, on_command)?;
    tokio::spawn(report_loop(player, service));
    Ok(())
}

/// 系统媒体控件命令 → 播放控制。
fn handle_command(player: &PlayerCore, cmd: MediaCommand) {
    let audio = player.audio();
    match cmd {
        MediaCommand::Play => audio.resume(),
        MediaCommand::Pause => audio.pause(),
        MediaCommand::Toggle => {
            if audio.snapshot().playing {
                audio.pause();
            } else {
                audio.resume();
            }
        }
        MediaCommand::Next => player.next_song(),
        MediaCommand::Previous => player.prev_or_restart(),
        MediaCommand::Stop => audio.stop(),
        MediaCommand::SeekForward(delta) => {
            let pos = audio.snapshot().position_ms;
            audio.seek(pos.saturating_add(dur_ms(delta)));
        }
        MediaCommand::SeekBackward(delta) => {
            let pos = audio.snapshot().position_ms;
            audio.seek(pos.saturating_sub(dur_ms(delta)));
        }
        MediaCommand::SetPosition(at) => audio.seek(dur_ms(at)),
    }
}

/// `Duration` → 毫秒(u64),溢出饱和到 `u64::MAX`。
fn dur_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// 歌词四路(逐字 / 行级原文 / 翻译 / 罗马音)的「是否非空」指纹。
///
/// 各路异步陆续到达,任一路从无到有都要重发 metadata,否则晚到的那路显示端收不到
/// (切歌后 quickshell 只有 ~10s 轮询窗口)。用它和换歌一起判定是否重报。
#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct LyricsPresence {
    /// 逐字原文(`mineral:words`)是否非空。
    words: bool,

    /// 行级原文(`xesam:asText`)是否非空。
    lrc: bool,

    /// 行级翻译(`mineral:translation`)是否非空。
    translation: bool,

    /// 行级罗马音(`mineral:romanization`)是否非空。
    romanization: bool,
}

impl LyricsPresence {
    /// 从当前歌词快照算出四路存在性。`None`(还没拉到歌词)= 全空。
    fn of(lyrics: Option<&Lyrics>) -> Self {
        match lyrics {
            None => Self::default(),
            Some(l) => Self {
                words: !l.words.is_empty(),
                lrc: !l.lrc.is_empty(),
                translation: !l.translation.is_empty(),
                romanization: !l.romanization.is_empty(),
            },
        }
    }
}

/// 实际位置偏离「线性外推预期」超过 [`SEEK_THRESHOLD_MS`] → 判定为 seek 跳变。
///
/// 正常播放时预期 = 上次位置 + 流逝时间(速率恒 1),实际≈预期;暂停时预期不前进。
/// seek / `SetPosition` 让位置非线性跳变,偏差远超阈值。
fn looks_like_seek(prev_ms: u64, actual_ms: u64, elapsed_ms: u64, was_playing: bool) -> bool {
    let expected = if was_playing {
        prev_ms.saturating_add(elapsed_ms)
    } else {
        prev_ms
    };
    actual_ms.abs_diff(expected) > SEEK_THRESHOLD_MS
}

/// 周期把 metadata + playback 上报给系统媒体控件。
///
/// metadata 在换歌、或任一路歌词从无到有时重报;playback(状态 + 进度)每 tick 上报;
/// 检测到非线性位置跳变(seek)时补发 `Seeked` 信号(外推型客户端靠它重置基准)。
async fn report_loop(player: PlayerCore, service: MediaService) {
    let mut tick = tokio::time::interval(Duration::from_millis(REPORT_INTERVAL_MS));
    let mut last_song_id = Option::<SongId>::None;
    let mut last_presence = LyricsPresence::default();
    let mut last_pos = Option::<u64>::None;
    let mut last_tick = Instant::now();
    let mut last_playing = false;
    loop {
        tick.tick().await;
        let now = Instant::now();
        let snap = player.snapshot();
        let audio = player.audio().snapshot();

        // 歌词在 channel 层已结构化清洗,这里只在确实要重发 metadata 时才序列化:
        // 行级走标准 LRC,逐字走 quickshell 约定的 JSON(见 build_now_playing)。
        let cur_id = snap.current_song.as_ref().map(|s| s.id.clone());
        let presence = LyricsPresence::of(snap.current_lyrics.as_ref());
        if cur_id != last_song_id || presence != last_presence {
            if let Some(song) = &snap.current_song {
                let now_playing = build_now_playing(song, snap.current_lyrics.as_ref());
                if let Err(e) = service.set_now_playing(&now_playing) {
                    mineral_log::warn!(target: "media", "set_now_playing failed: {e}");
                }
            }
            last_song_id = cur_id;
            last_presence = presence;
        }

        // 检测 seek:report_loop 是 snapshot 轮询拿不到事件,靠线性外推对比判定跳变,
        // 跳变时补发 Seeked(只在有当前歌时;首 tick last_pos=None 不判定)。
        if snap.current_song.is_some() {
            let elapsed_ms =
                u64::try_from(now.duration_since(last_tick).as_millis()).unwrap_or(u64::MAX);
            if let Some(prev) = last_pos
                && looks_like_seek(prev, audio.position_ms, elapsed_ms, last_playing)
                && let Err(e) = service.notify_seek(Duration::from_millis(audio.position_ms))
            {
                mineral_log::warn!(target: "media", "notify_seek failed: {e}");
            }
        }
        last_pos = Some(audio.position_ms);
        last_tick = now;
        last_playing = audio.playing;

        let (state, position) = playback_of(&snap, &audio);
        if let Err(e) = service.set_playback(state, position) {
            mineral_log::warn!(target: "media", "set_playback failed: {e}");
        }
    }
}

/// 从快照算出上报用的 [`PlaybackState`] 与进度;无当前歌 = `Stopped`。
fn playback_of(snap: &PlayerSnapshot, audio: &AudioSnapshot) -> (PlaybackState, Option<Duration>) {
    if snap.current_song.is_none() {
        return (PlaybackState::Stopped, None);
    }
    let state = if audio.playing {
        PlaybackState::Playing
    } else {
        PlaybackState::Paused
    };
    (state, Some(Duration::from_millis(audio.position_ms)))
}

/// [`Song`] + 结构化歌词 → 上报用的 [`NowPlaying`]。
///
/// 歌词原样以结构化形式塞进 [`NowPlaying`],序列化(LRC / JSON)推迟到 MPRIS 适配层
/// (mineral-media)写 metadata 的最边界做。无歌词时各路为空。
fn build_now_playing(song: &Song, lyrics: Option<&Lyrics>) -> NowPlaying {
    let artist = if song.artists.is_empty() {
        None
    } else {
        let names = song
            .artists
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>();
        Some(names.join(" / "))
    };
    let builder = NowPlaying::builder()
        .title(Some(song.name.clone()))
        .artist(artist)
        .album(song.album.as_ref().map(|a| a.name.clone()))
        .cover_url(song.cover_url.as_ref().map(cover_to_url))
        .duration(Some(Duration::from_millis(song.duration_ms)));
    match lyrics {
        None => builder.build(),
        Some(l) => builder
            .lrc(l.lrc.clone())
            .words(l.words.clone())
            .translation(l.translation.clone())
            .romanization(l.romanization.clone())
            .build(),
    }
}

/// 封面 [`MediaUrl`] → MPRIS `artUrl` 字符串(本地路径补 `file://` 前缀)。
fn cover_to_url(url: &MediaUrl) -> String {
    match url.as_remote() {
        Some(remote) => remote.as_str().to_owned(),
        None => format!("file://{url}"),
    }
}

#[cfg(test)]
mod tests {
    use super::looks_like_seek;

    #[test]
    fn normal_playback_not_seek() {
        // 一个 tick(200ms)位置正好前进 200ms → 预期=实际,不判 seek。
        assert!(!looks_like_seek(
            /*prev_ms*/ 10_000, /*actual_ms*/ 10_200, /*elapsed_ms*/ 200,
            /*was_playing*/ true
        ));
    }

    #[test]
    fn jitter_within_threshold_not_seek() {
        // tick 抖动 / 采样误差(实际比预期少 180ms)< 1s 阈值 → 不判 seek。
        assert!(!looks_like_seek(10_000, 10_380, 200, true));
    }

    #[test]
    fn seek_forward_is_seek() {
        // 从 10s 跳到 70s,远超阈值 → 判 seek。
        assert!(looks_like_seek(10_000, 70_000, 200, true));
    }

    #[test]
    fn seek_backward_is_seek() {
        assert!(looks_like_seek(60_000, 5_000, 200, true));
    }

    #[test]
    fn paused_position_steady_not_seek() {
        // 暂停:预期不前进(was_playing=false),位置也没动 → 不判 seek,即便流逝很久。
        assert!(!looks_like_seek(10_000, 10_000, 5_000, false));
    }
}

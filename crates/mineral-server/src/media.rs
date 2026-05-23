//! 系统媒体服务(MPRIS)接入:把 [`PlayerCore`] 的播放状态上报给系统媒体控件,
//! 把控件发来的命令转成播放控制。
//!
//! 只在 daemon(常驻 server)里启用 —— 系统媒体控件控制的是常驻播放,关掉 TUI
//! 仍然有效。in-proc 模式(TUI 自起 server)不调用本模块,避免多实例抢同一条
//! D-Bus 总线名。

use std::sync::Arc;
use std::time::Duration;

use mineral_audio::AudioSnapshot;
use mineral_media::{MediaCommand, MediaConfig, MediaService, NowPlaying, PlaybackState};
use mineral_model::{MediaUrl, Song, SongId};
use mineral_protocol::PlayerSnapshot;

use crate::player::PlayerCore;

/// 状态上报循环的间隔。200ms 让 Position 跟手,显示端按它做逐行歌词同步才平滑
const REPORT_INTERVAL_MS: u64 = 200;

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

/// 周期把 metadata + playback 上报给系统媒体控件。
///
/// metadata 仅在当前歌变化时重报;playback(状态 + 进度)每 tick 上报。
async fn report_loop(player: PlayerCore, service: MediaService) {
    let mut tick = tokio::time::interval(Duration::from_millis(REPORT_INTERVAL_MS));
    let mut last_song_id = Option::<SongId>::None;
    let mut last_has_lyrics = false;
    loop {
        tick.tick().await;
        let snap = player.snapshot();
        let audio = player.audio().snapshot();

        // 歌词是异步拉取的,常比 current_song 晚到:song 没变但歌词从无到有时,
        // 也要重发 metadata 把 xesam:asText 补上(显示端切歌后有 ~10s 轮询窗口)。
        let cur_id = snap.current_song.as_ref().map(|s| s.id.clone());
        let cur_lyrics = snap
            .current_lyrics
            .as_ref()
            .and_then(|l| l.lrc.as_deref())
            .map(clean_lrc);
        let has_lyrics = cur_lyrics.is_some();
        if cur_id != last_song_id || has_lyrics != last_has_lyrics {
            if let Some(song) = &snap.current_song {
                let now_playing = build_now_playing(song, cur_lyrics);
                if let Err(e) = service.set_now_playing(&now_playing) {
                    mineral_log::warn!(target: "media", "set_now_playing failed: {e}");
                }
            }
            last_song_id = cur_id;
            last_has_lyrics = has_lyrics;
        }

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

/// [`Song`] + LRC 歌词 → 上报用的 [`NowPlaying`]。
fn build_now_playing(song: &Song, lyrics: Option<String>) -> NowPlaying {
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
    NowPlaying::builder()
        .title(Some(song.name.clone()))
        .artist(artist)
        .album(song.album.as_ref().map(|a| a.name.clone()))
        .cover_url(song.cover_url.as_ref().map(cover_to_url))
        .duration(Some(Duration::from_millis(song.duration_ms)))
        .lyrics(lyrics)
        .build()
}

/// 清洗网易原始 lrc:丢弃混入的 YRC 逐字 JSON 行(`{...}`),只留标准 LRC 文本。
///
/// 网易部分歌曲的 `lrc.lyric` 会把作词/作曲等元信息用 YRC JSON 行塞在开头,
/// 显示端按 `[mm:ss.xx]` 解析时这些行是噪音,这里直接滤掉。
fn clean_lrc(raw: &str) -> String {
    raw.lines()
        .filter(|line| !line.trim_start().starts_with('{'))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 封面 [`MediaUrl`] → MPRIS `artUrl` 字符串(本地路径补 `file://` 前缀)。
fn cover_to_url(url: &MediaUrl) -> String {
    match url.as_remote() {
        Some(remote) => remote.as_str().to_owned(),
        None => format!("file://{url}"),
    }
}

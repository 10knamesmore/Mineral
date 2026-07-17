//! 系统媒体服务(MPRIS)接入:把 [`PlayerCore`] 的播放状态上报给系统媒体控件,
//! 把控件发来的命令转成播放控制。
//!
//! 只在 daemon(常驻 server)里启用 —— 系统媒体控件控制的是常驻播放,关掉 TUI
//! 仍然有效。in-proc 模式(TUI 自起 server)不调用本模块,避免多实例抢同一条
//! D-Bus 总线名。

use std::sync::Arc;
use std::time::{Duration, Instant};

use mineral_audio::AudioSnapshot;
use mineral_media::{LoopMode, MediaCommand, MediaConfig, MediaService, NowPlaying, PlaybackState};
use mineral_model::{Lyrics, MediaUrl, Song, SongId};
use mineral_protocol::{PlayMode, Repeat};

use crate::player::PlayerCore;

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

/// 系统媒体控件命令 → 播放控制。走 PlayerCore 的 transport 方法(执行 + 埋点同点):
/// 媒体键是用户按的,actor=User,与界面按键同样入库。
fn handle_command(player: &PlayerCore, cmd: MediaCommand) {
    use mineral_stats::Actor;
    let audio = player.audio();
    match cmd {
        MediaCommand::Play => player.resume_playback(Actor::User),
        MediaCommand::Pause => player.pause_playback(Actor::User),
        MediaCommand::Toggle => player.toggle_playback(Actor::User),
        MediaCommand::Next => player.next_song(Actor::User),
        MediaCommand::Previous => player.prev_or_restart(Actor::User),
        MediaCommand::Stop => player.stop_playback(),
        MediaCommand::SeekForward(delta) => {
            let pos = audio.snapshot().position_ms;
            player.seek_playback(pos.saturating_add(dur_ms(delta)), Actor::User);
        }
        MediaCommand::SeekBackward(delta) => {
            let pos = audio.snapshot().position_ms;
            player.seek_playback(pos.saturating_sub(dur_ms(delta)), Actor::User);
        }
        MediaCommand::SetPosition(at) => player.seek_playback(dur_ms(at), Actor::User),
        // 控件只写单个维度;读当前 PlayMode、改对应维度、塌缩回四档(report_loop 随即回报真实档)。
        MediaCommand::SetShuffle(on) => {
            let mode = player.with_state(|st| st.play_mode);
            player.set_play_mode(mode.with_shuffle(on), Actor::User);
        }
        MediaCommand::SetLoop(loop_mode) => {
            let mode = player.with_state(|st| st.play_mode);
            player.set_play_mode(mode.with_repeat(loop_to_repeat(loop_mode)), Actor::User);
        }
    }
}

/// MPRIS `LoopStatus`(平台无关 [`LoopMode`])→ [`Repeat`] 维度。
fn loop_to_repeat(mode: LoopMode) -> Repeat {
    match mode {
        LoopMode::None => Repeat::Off,
        LoopMode::Track => Repeat::One,
        LoopMode::Playlist => Repeat::All,
    }
}

/// [`Repeat`] 维度 → MPRIS `LoopStatus`(平台无关 [`LoopMode`])。
fn repeat_to_loop(repeat: Repeat) -> LoopMode {
    match repeat {
        Repeat::Off => LoopMode::None,
        Repeat::One => LoopMode::Track,
        Repeat::All => LoopMode::Playlist,
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
                words: mineral_model::has_words(&l.lines),
                lrc: mineral_model::has_timed(&l.lines),
                translation: l.has_translation(),
                romanization: l.has_romanization(),
            },
        }
    }
}

/// 实际位置偏离「线性外推预期」超过 `threshold_ms` → 判定为 seek 跳变。
///
/// 正常播放时预期 = 上次位置 + 流逝时间(速率恒 1),实际≈预期;暂停时预期不前进。
/// seek / `SetPosition` 让位置非线性跳变,偏差远超阈值(配置 `daemon.seek_threshold_ms`)。
fn looks_like_seek(
    prev_ms: u64,
    actual_ms: u64,
    elapsed_ms: u64,
    was_playing: bool,
    threshold_ms: u64,
) -> bool {
    let expected = if was_playing {
        prev_ms.saturating_add(elapsed_ms)
    } else {
        prev_ms
    };
    actual_ms.abs_diff(expected) > threshold_ms
}

/// 周期把 metadata + playback 上报给系统媒体控件。
///
/// metadata 在换歌、或任一路歌词从无到有时重报;playback(状态 + 进度)每 tick 上报;
/// 检测到非线性位置跳变(seek)时补发 `Seeked` 信号(外推型客户端靠它重置基准)。
async fn report_loop(player: PlayerCore, service: MediaService) {
    let mut tick = tokio::time::interval(Duration::from_millis(player.media_report_interval_ms()));
    let seek_threshold_ms = player.media_seek_threshold_ms();
    let mut last_song_id = Option::<SongId>::None;
    let mut last_presence = LyricsPresence::default();
    let mut last_pos = Option::<u64>::None;
    let mut last_tick = Instant::now();
    let mut last_playing = false;
    let mut last_play_mode = Option::<PlayMode>::None;
    loop {
        tick.tick().await;
        let now = Instant::now();
        // in-process 直读 State 需要的三个字段(歌 + 歌词 + 模式),不再拉含整个
        // queue 的全量快照(queue 这里用不上,clone 它纯浪费)。
        let (current_song, current_lyrics, play_mode) = player.with_state(|st| {
            (
                st.current_song.clone(),
                st.current_lyrics.clone(),
                st.play_mode,
            )
        });
        let audio = player.audio().snapshot();

        // 歌词在 channel 层已结构化清洗,这里只在确实要重发 metadata 时才序列化:
        // 行级走标准 LRC,逐字走 quickshell 约定的 JSON(见 build_now_playing)。
        let cur_id = current_song.as_ref().map(|s| s.id.clone());
        let presence = LyricsPresence::of(current_lyrics.as_ref());
        let song_changed = cur_id != last_song_id;
        if song_changed || presence != last_presence {
            if let Some(song) = &current_song {
                let now_playing = build_now_playing(song, current_lyrics.as_ref());
                if let Err(e) = service.set_now_playing(&now_playing) {
                    mineral_log::warn!(target: "media", error = mineral_log::chain(&e), "set_now_playing failed");
                }
                // 新歌:异步拉封面字节,到手后补进系统媒体中心(其余平台用 cover_url 字符串,无需字节)。
                #[cfg(target_os = "macos")]
                if song_changed {
                    spawn_artwork_fetch(service.clone(), song.cover_url.clone());
                }
            }
            last_song_id = cur_id;
            last_presence = presence;
        }

        // 检测 seek:report_loop 是 snapshot 轮询拿不到事件,靠线性外推对比判定跳变,
        // 跳变时补发 Seeked(只在有当前歌时;首 tick last_pos=None 不判定)。
        if current_song.is_some() {
            let elapsed_ms =
                u64::try_from(now.duration_since(last_tick).as_millis()).unwrap_or(u64::MAX);
            if let Some(prev) = last_pos
                && looks_like_seek(
                    prev,
                    audio.position_ms,
                    elapsed_ms,
                    last_playing,
                    seek_threshold_ms,
                )
                && let Err(e) = service.notify_seek(Duration::from_millis(audio.position_ms))
            {
                mineral_log::warn!(target: "media", error = mineral_log::chain(&e), "notify_seek failed");
            }
        }
        last_pos = Some(audio.position_ms);
        last_tick = now;
        last_playing = audio.playing;

        // PlayMode 变化时把两维度回写 MPRIS Shuffle/LoopStatus(自动发 PropertiesChanged);
        // 首 tick(last_play_mode=None)必报一次,纠正 mpris-server 的默认 Off/None。
        if last_play_mode != Some(play_mode) {
            if let Err(e) = service.set_shuffle(play_mode.shuffle()) {
                mineral_log::warn!(target: "media", error = mineral_log::chain(&e), "set_shuffle failed");
            }
            if let Err(e) = service.set_loop(repeat_to_loop(play_mode.repeat())) {
                mineral_log::warn!(target: "media", error = mineral_log::chain(&e), "set_loop failed");
            }
            last_play_mode = Some(play_mode);
        }

        let (state, position) = playback_of(current_song.as_ref(), &audio);
        if let Err(e) = service.set_playback(state, position) {
            mineral_log::warn!(target: "media", error = mineral_log::chain(&e), "set_playback failed");
        }
    }
}

/// 从当前歌 + 音频快照算出上报用的 [`PlaybackState`] 与进度;无当前歌 = `Stopped`。
fn playback_of(
    current_song: Option<&Song>,
    audio: &AudioSnapshot,
) -> (PlaybackState, Option<Duration>) {
    if current_song.is_none() {
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
        .duration(song.duration_ms.map(Duration::from_millis));
    match lyrics {
        None => builder.build(),
        // 翻译 / 罗马音轨从合并行重建:时间戳取原文行的,与 asText 严格对齐。
        Some(l) => builder
            .original(l.lines.clone())
            .translation(l.translation_lines())
            .romanization(l.romanization_lines())
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

/// 异步拉取封面字节并补进系统媒体中心(macOS)。失败只 warn / debug,不影响播放。
///
/// 系统媒体中心要的是真图片数据(不能只给 URL),故新歌时单独拉一次字节投给后端。
#[cfg(target_os = "macos")]
fn spawn_artwork_fetch(service: MediaService, cover: Option<MediaUrl>) {
    let Some(cover) = cover else {
        return;
    };
    tokio::spawn(async move {
        match load_cover_bytes(&cover).await {
            Ok(bytes) => {
                if let Err(e) = service.set_artwork(&bytes) {
                    mineral_log::warn!(target: "media", error = mineral_log::chain(&e), "set_artwork failed");
                }
            }
            Err(e) => {
                mineral_log::debug!(target: "media", error = mineral_log::chain(&e), "拉取封面字节失败,跳过 artwork");
            }
        }
    });
}

/// 取封面字节:远程走一次性 HTTP GET,本地直接读文件。
#[cfg(target_os = "macos")]
async fn load_cover_bytes(cover: &MediaUrl) -> color_eyre::Result<Vec<u8>> {
    use color_eyre::eyre::{WrapErr, eyre};

    if let Some(remote) = cover.as_remote() {
        let client = reqwest::Client::builder()
            .build()
            .wrap_err("构造封面 http client")?;
        let bytes = client
            .get(remote.as_str())
            .send()
            .await
            .wrap_err("发起封面请求")?
            .error_for_status()
            .wrap_err("封面响应状态码非 2xx")?
            .bytes()
            .await
            .wrap_err("读取封面字节")?;
        return Ok(bytes.to_vec());
    }
    if let Some(path) = cover.as_local() {
        return tokio::fs::read(path)
            .await
            .wrap_err_with(|| format!("读取本地封面 {}", path.display()));
    }
    Err(eyre!("cover url 既非远程也非本地"))
}

#[cfg(test)]
mod tests {
    use super::looks_like_seek;

    #[test]
    fn normal_playback_not_seek() {
        // 一个 tick(200ms)位置正好前进 200ms → 预期=实际,不判 seek。
        assert!(!looks_like_seek(
            /*prev_ms*/ 10_000, /*actual_ms*/ 10_200, /*elapsed_ms*/ 200,
            /*was_playing*/ true, /*threshold_ms*/ 1000
        ));
    }

    #[test]
    fn jitter_within_threshold_not_seek() {
        // tick 抖动 / 采样误差(实际比预期少 180ms)< 1s 阈值 → 不判 seek。
        assert!(!looks_like_seek(
            10_000, 10_380, 200, true, /*threshold_ms*/ 1000
        ));
    }

    #[test]
    fn seek_forward_is_seek() {
        // 从 10s 跳到 70s,远超阈值 → 判 seek。
        assert!(looks_like_seek(
            10_000, 70_000, 200, true, /*threshold_ms*/ 1000
        ));
    }

    #[test]
    fn seek_backward_is_seek() {
        assert!(looks_like_seek(
            60_000, 5_000, 200, true, /*threshold_ms*/ 1000
        ));
    }

    #[test]
    fn paused_position_steady_not_seek() {
        // 暂停:预期不前进(was_playing=false),位置也没动 → 不判 seek,即便流逝很久。
        assert!(!looks_like_seek(
            10_000, 10_000, 5_000, false, /*threshold_ms*/ 1000
        ));
    }
}

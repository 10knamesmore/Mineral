//! Linux MediaService:基于 mpris-server(zbus 官方)的 MPRIS 实现。
//!
//! mpris-server 是 async,且其 server task 为 `!Send`(`LocalServerRunTask`),
//! 不能直接丢进 daemon 的多线程 tokio runtime。这里起一个**专属线程**,在它的
//! current-thread runtime + `LocalSet` 里 build player、`spawn_local` 它的 run
//! task、并消费状态更新。对外仍是同步 API:状态更新经 channel 投递,命令经
//! `on_command` 回调(在专属线程触发)回传。
//!
//! 选 mpris-server 而非 souvlaki 的原因:它能设任意 metadata 字段,我们要往
//! `xesam:asText` 塞 LRC 歌词,souvlaki 的固定 5 字段做不到。

use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::eyre;
use mineral_model::WordLyric;
use mpris_server::{Metadata, PlaybackStatus, Player, Time};
use serde::Serialize;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::command::MediaCommand;
use crate::config::MediaConfig;
use crate::state::{NowPlaying, PlaybackState};

/// 主线程 → MPRIS 专属线程的状态更新消息。
enum Update {
    /// 重设当前曲目元数据(含 `xesam:asText` 歌词)。
    Metadata(NowPlaying),

    /// 重设播放状态与进度。
    Playback {
        /// 播放 / 暂停 / 停止。
        status: PlaybackState,

        /// 当前进度;`None` 表示不更新位置。
        position: Option<Duration>,
    },
}

/// 系统媒体服务句柄(Linux = MPRIS via mpris-server)。
pub struct MediaService {
    /// 向专属线程投递状态更新。
    tx: UnboundedSender<Update>,
}

impl MediaService {
    /// 起 MPRIS 专属线程,注册控件 + attach 命令回调,等到注册完成才返回。
    ///
    /// # Params:
    ///   - `config`: D-Bus 名后缀(同时用作 identity)与显示名。
    ///   - `on_command`: 收到系统媒体控件命令时回调,在专属线程触发。
    ///
    /// # Return:
    ///   注册失败(无 D-Bus session 等)返回 `Err`。
    pub fn spawn(
        config: &MediaConfig,
        on_command: Arc<dyn Fn(MediaCommand) + Send + Sync>,
    ) -> color_eyre::Result<Self> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Update>();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<color_eyre::Result<()>>();
        let dbus_name = config.dbus_name.clone();
        let identity = config.display_name.clone();
        std::thread::Builder::new()
            .name("mineral-mpris".to_owned())
            .spawn(move || run_thread(&dbus_name, &identity, &on_command, rx, &ready_tx))
            .map_err(|e| eyre!("spawn mpris thread: {e}"))?;
        match ready_rx.recv() {
            Ok(result) => result.map(|()| Self { tx }),
            Err(e) => Err(eyre!("mpris thread exited before ready: {e}")),
        }
    }

    /// 上报当前曲目元数据(含歌词)。
    pub fn set_now_playing(&self, now_playing: &NowPlaying) -> color_eyre::Result<()> {
        self.tx
            .send(Update::Metadata(now_playing.clone()))
            .map_err(|e| eyre!("mpris thread gone: {e}"))
    }

    /// 上报播放状态与进度。
    pub fn set_playback(
        &self,
        state: PlaybackState,
        position: Option<Duration>,
    ) -> color_eyre::Result<()> {
        self.tx
            .send(Update::Playback {
                status: state,
                position,
            })
            .map_err(|e| eyre!("mpris thread gone: {e}"))
    }
}

/// 专属线程主体:current-thread runtime + `LocalSet`,build player 后消费更新。
fn run_thread(
    dbus_name: &str,
    identity: &str,
    on_command: &Arc<dyn Fn(MediaCommand) + Send + Sync>,
    mut rx: UnboundedReceiver<Update>,
    ready_tx: &std::sync::mpsc::Sender<color_eyre::Result<()>>,
) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = ready_tx.send(Err(eyre!("build mpris runtime: {e}")));
            return;
        }
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let player = match build_player(dbus_name, identity, on_command).await {
            Ok(p) => p,
            Err(e) => {
                let _ = ready_tx.send(Err(e));
                return;
            }
        };
        tokio::task::spawn_local(player.run());
        let _ = ready_tx.send(Ok(()));
        while let Some(update) = rx.recv().await {
            apply_update(&player, update).await;
        }
    });
}

/// build mpris-server `Player` 并接好命令回调。
async fn build_player(
    dbus_name: &str,
    identity: &str,
    on_command: &Arc<dyn Fn(MediaCommand) + Send + Sync>,
) -> color_eyre::Result<Player> {
    let player = Player::builder(dbus_name)
        .identity(identity)
        .can_play(true)
        .can_pause(true)
        .can_go_next(true)
        .can_go_previous(true)
        .can_seek(true)
        .can_control(true)
        .build()
        .await
        .map_err(|e| eyre!("build mpris player: {e}"))?;
    wire_handlers(&player, on_command);
    Ok(player)
}

/// 把 mpris-server 的命令信号接到平台无关的 [`MediaCommand`] 回调。
fn wire_handlers(player: &Player, on_command: &Arc<dyn Fn(MediaCommand) + Send + Sync>) {
    let cb = Arc::clone(on_command);
    player.connect_play(move |_| cb(MediaCommand::Play));
    let cb = Arc::clone(on_command);
    player.connect_pause(move |_| cb(MediaCommand::Pause));
    let cb = Arc::clone(on_command);
    player.connect_play_pause(move |_| cb(MediaCommand::Toggle));
    let cb = Arc::clone(on_command);
    player.connect_next(move |_| cb(MediaCommand::Next));
    let cb = Arc::clone(on_command);
    player.connect_previous(move |_| cb(MediaCommand::Previous));
    let cb = Arc::clone(on_command);
    player.connect_stop(move |_| cb(MediaCommand::Stop));

    let cb = Arc::clone(on_command);
    player.connect_seek(move |_, offset: Time| {
        let micros = offset.as_micros();
        if micros >= 0 {
            cb(MediaCommand::SeekForward(micros_to_duration(micros)));
        } else {
            cb(MediaCommand::SeekBackward(micros_to_duration(
                micros.saturating_neg(),
            )));
        }
    });

    let cb = Arc::clone(on_command);
    player.connect_set_position(move |_, _track, position: Time| {
        cb(MediaCommand::SetPosition(micros_to_duration(
            position.as_micros(),
        )));
    });
}

/// 应用一条状态更新到 player(set 失败只 warn,不影响播放)。
async fn apply_update(player: &Player, update: Update) {
    match update {
        Update::Metadata(now_playing) => {
            if let Err(e) = player.set_metadata(build_metadata(&now_playing)).await {
                mineral_log::warn!(target: "media", "mpris set_metadata: {e}");
            }
        }
        Update::Playback { status, position } => {
            if let Err(e) = player.set_playback_status(to_status(status)).await {
                mineral_log::warn!(target: "media", "mpris set_playback_status: {e}");
            }
            if let Some(p) = position {
                // set_position 是同步的:只更新 Position 属性内部值,不发
                // PropertiesChanged(MPRIS 规范);客户端按需轮询读取。
                player.set_position(duration_to_time(p));
            }
        }
    }
}

/// [`NowPlaying`] → mpris-server `Metadata`。
///
/// 结构化歌词在这里(写 MPRIS 的最边界)才序列化:行级原文 / 翻译 / 罗马音走标准 LRC,
/// 逐字原文走JSON([`serialize_words`])。某路为空就不 set 对应 key
/// (key 不存在即代表该轨无数据,显示端按 逐字 → 行级 → 无 降级)。
fn build_metadata(now_playing: &NowPlaying) -> Metadata {
    let mut builder = Metadata::builder();
    if let Some(title) = &now_playing.title {
        builder = builder.title(title.clone());
    }
    if let Some(artist) = &now_playing.artist {
        builder = builder.artist([artist.clone()]);
    }
    if let Some(album) = &now_playing.album {
        builder = builder.album(album.clone());
    }
    if let Some(cover) = &now_playing.cover_url {
        builder = builder.art_url(cover.clone());
    }
    if let Some(duration) = now_playing.duration {
        builder = builder.length(duration_to_time(duration));
    }
    let mut metadata = builder.build();
    if !now_playing.lrc.is_empty() {
        let _ = metadata.set("xesam:asText", Some(now_playing.lrc.to_lrc_string()));
    }
    if let Some(json) = serialize_words(&now_playing.words) {
        let _ = metadata.set("mineral:words", Some(json));
    }
    if !now_playing.translation.is_empty() {
        let _ = metadata.set(
            "mineral:translation",
            Some(now_playing.translation.to_lrc_string()),
        );
    }
    if !now_playing.romanization.is_empty() {
        let _ = metadata.set(
            "mineral:romanization",
            Some(now_playing.romanization.to_lrc_string()),
        );
    }
    metadata
}

/// `mineral:words` JSON 的一行(字段名即 quickshell 契约,勿改 / 勿缩写)。
#[derive(Serialize)]
struct WordsLineDto {
    /// 行起始绝对毫秒。
    start: u64,

    /// 该行的字单元,按时间升序。
    words: Vec<WordCellDto>,
}

/// `mineral:words` JSON 的一个字单元(字段名即 quickshell 契约,勿改 / 勿缩写)。
#[derive(Serialize)]
struct WordCellDto {
    /// 字起始绝对毫秒。
    start: u64,

    /// 字持续毫秒(wipe 高亮要用,行末字 / 间隙无法靠下一字推算,必须显式给)。
    duration: u64,

    /// 字面文本,原样保留前后空格(显示端直接拼成行)。
    text: String,
}

/// 逐字歌词 → `mineral:words` 的 JSON 字符串;空歌词返回 `None`(不发该 key)。
fn serialize_words(words: &WordLyric) -> Option<String> {
    if words.is_empty() {
        return None;
    }
    let dto = words
        .iter()
        .map(|line| WordsLineDto {
            start: line.start_ms,
            words: line
                .words
                .iter()
                .map(|w| WordCellDto {
                    start: w.start_ms,
                    duration: w.dur_ms,
                    text: w.text.clone(),
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    serde_json::to_string(&dto).ok()
}

/// [`PlaybackState`] → mpris-server `PlaybackStatus`。
fn to_status(state: PlaybackState) -> PlaybackStatus {
    match state {
        PlaybackState::Playing => PlaybackStatus::Playing,
        PlaybackState::Paused => PlaybackStatus::Paused,
        PlaybackState::Stopped => PlaybackStatus::Stopped,
    }
}

/// `Duration` → mpris-server `Time`(微秒),溢出饱和到 `i64::MAX`。
fn duration_to_time(d: Duration) -> Time {
    let micros = i64::try_from(d.as_micros()).unwrap_or(i64::MAX);
    Time::from_micros(micros)
}

/// 微秒(i64,非负)→ `Duration`。
fn micros_to_duration(micros: i64) -> Duration {
    Duration::from_micros(u64::try_from(micros).unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::serialize_words;
    use color_eyre::eyre::eyre;
    use mineral_model::{Word, WordLine, WordLyric};
    use serde_json::Value;

    fn word(start_ms: u64, dur_ms: u64, text: &str) -> Word {
        Word {
            start_ms,
            dur_ms,
            text: text.to_owned(),
        }
    }

    #[test]
    fn empty_words_serialize_to_none() {
        // 空逐字 → None,build_metadata 据此不发 mineral:words key。
        assert_eq!(serialize_words(&WordLyric::default()), None);
    }

    #[test]
    fn words_serialize_to_quickshell_schema() -> color_eyre::Result<()> {
        // 字段名 / 层级 / 整数毫秒 / 保留空格 —— 严格对齐 quickshell 契约。
        let lyric: WordLyric = vec![WordLine {
            start_ms: 11_350,
            dur_ms: 1020,
            words: vec![word(11_350, 300, "How "), word(11_650, 720, "will")],
        }]
        .into();
        let json = serialize_words(&lyric).ok_or_else(|| eyre!("expected Some json"))?;
        let v: Value = serde_json::from_str(&json)?;

        // 顶层数组,行有 start + words,无行级 duration 字段。
        let line = v.get(0).ok_or_else(|| eyre!("missing line 0"))?;
        assert_eq!(line.get("start").and_then(Value::as_u64), Some(11_350));
        assert!(line.get("duration").is_none());
        // 字单元字段为 start/duration/text,整数毫秒。
        let w0 = line
            .get("words")
            .and_then(|w| w.get(0))
            .ok_or_else(|| eyre!("missing word 0"))?;
        assert_eq!(w0.get("start").and_then(Value::as_u64), Some(11_350));
        assert_eq!(w0.get("duration").and_then(Value::as_u64), Some(300));
        // text 原样保留尾随空格(显示端直接拼)。
        assert_eq!(w0.get("text").and_then(Value::as_str), Some("How "));
        Ok(())
    }
}

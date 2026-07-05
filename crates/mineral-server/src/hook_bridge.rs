//! server 侧脚本拦截桥:`before_play` / `before_download` 的唯一插桩面。
//!
//! 职责边界:把播放 / 下载链路的拦截窗口接到脚本线程
//! ([`ScriptSender::intercept`]),并把裁决落回执行面。无脚本线程时
//! 走完全同步的原路径(零行为变化);拦截一切异常(超时 / 线程退出 /
//! Lua 错误)都收敛为放行。
//!
//! 已知取舍(C2/C3 范围):本地命中(`resolve_local`)与 gapless 预排
//! 路径**不**过 hook —— 前者改写语义不成立(用户自己的文件),后者
//! decoder 已预排、改写窗口不存在;fallback 场景的主路径(远端解析后
//! 起播 / 下载取链后)全覆盖。

use mineral_model::{PlayUrl, Song};
use mineral_script::{HookContext, HookDecision, HookKind, ScriptSender};

use crate::download;
use crate::player::PlayerCore;

/// 拦截门:播放 / 下载编排持有的脚本拦截入口。
///
/// 无脚本(构造时 `sender` 缺席或未挂线程)恒放行且不产生异步开销;
/// 测试用 [`Self::disabled`] 构造永远放行的门。
pub(crate) struct HookGate {
    /// 脚本投递句柄;`None` = 无脚本,永远放行。
    sender: Option<ScriptSender>,

    /// 软超时(配置 `script.hook_timeout_ms`)。
    timeout: std::time::Duration,
}

impl HookGate {
    /// 永远放行的门(单测直调 `download_song` 时脱脚本依赖用)。
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            sender: None,
            timeout: std::time::Duration::ZERO,
        }
    }

    /// 接指定脚本线程的门(单测注入真 hook 用)。
    ///
    /// # Params:
    ///   - `sender`: 脚本投递句柄
    ///   - `timeout`: 软超时
    #[cfg(test)]
    pub(crate) fn with_sender(sender: ScriptSender, timeout: std::time::Duration) -> Self {
        Self {
            sender: Some(sender),
            timeout,
        }
    }

    /// 当前是否真有脚本线程可拦截(决定走同步快路还是异步拦截)。
    fn active(&self) -> Option<&ScriptSender> {
        self.sender.as_ref().filter(|s| s.is_attached())
    }

    /// 跑一次 `before_download` 拦截(下载链路在取链后、写盘前 await)。
    ///
    /// # Params:
    ///   - `song`: 待下载歌
    ///   - `original`: 取到的下载直链
    ///
    /// # Return:
    ///   裁决;无脚本时恒 [`HookDecision::Continue`]。
    pub(crate) async fn before_download(&self, song: &Song, original: &PlayUrl) -> HookDecision {
        let Some(sender) = self.active() else {
            return HookDecision::Continue;
        };
        sender
            .intercept(
                HookKind::BeforeDownload,
                HookContext::new(song.clone(), original.clone()),
                self.timeout,
            )
            .await
    }
}

impl PlayerCore {
    /// 构造拦截门(下载编排 / 播放插桩共用)。
    pub(crate) fn hook_gate(&self) -> HookGate {
        HookGate {
            sender: self.script_sender(),
            timeout: self.hook_timeout(),
        }
    }
}

/// `before_play` 插桩:远端播放 URL 就绪后、起播前。
///
/// 无脚本线程 → 同步直走原路径(play_capturing + 回填 play_url),与
/// 插桩前逐指令一致;有脚本 → spawn 异步拦截,裁决回来再起播(拦截
/// 窗口内的静音由软超时封顶)。
///
/// # Params:
///   - `player`: 播放核心
///   - `song`: 即将起播的歌
///   - `pu`: 解析出的播放 URL(本函数消费)
pub(crate) fn before_play(player: &PlayerCore, song: &Song, pu: PlayUrl) {
    let gate = player.hook_gate();
    let Some(sender) = gate.active().cloned() else {
        start_play(player, song, pu);
        return;
    };
    let player = player.clone();
    let song = song.clone();
    tokio::spawn(async move {
        let ctx = HookContext::new(song.clone(), pu.clone());
        let decision = sender
            .intercept(HookKind::BeforePlay, ctx, gate.timeout)
            .await;
        apply_play_decision(&player, &song, pu, decision);
    });
}

/// 把 `before_play` 的裁决落到播放执行面。
///
/// 拦截窗口内可能已切歌:不再是当前曲就整体丢弃(切歌路径早已 stop 音频,
/// 这里起播反而会复活一首旧歌)。
fn apply_play_decision(
    player: &PlayerCore,
    song: &Song,
    original: PlayUrl,
    decision: HookDecision,
) {
    let still_current = player.with_state(|st| {
        st.current_song
            .as_ref()
            .is_some_and(|current| current.id == song.id)
    });
    if !still_current {
        mineral_log::debug!(
            target: "script",
            song_id = song.id.as_str(),
            "拦截窗口内已切歌,丢弃裁决"
        );
        return;
    }
    match decision {
        HookDecision::Continue => start_play(player, song, original),
        HookDecision::Rewrite(spec) => {
            let mut effective = original;
            if let Some(url) = spec.new_url() {
                effective.url = url.clone();
            }
            if let Some(quality) = spec.new_quality() {
                effective.quality = quality;
            }
            // 解灰顶替进来的 url 同步带上其取流头(如 B站 baseUrl 需 `Referer`),否则播放 403。
            if let Some(headers) = spec.stream_headers() {
                effective.stream_headers = headers.to_vec();
            }
            mineral_log::info!(
                target: "script",
                song_id = song.id.as_str(),
                url = %effective.url,
                "before_play 改写播放 URL"
            );
            // 改写过的流不 capture 入缓存:缓存按 song_id+quality 入键,
            // 改写内容与原曲是否一致由脚本自负,污染缓存代价高;fallback 流每次现拉。
            player
                .audio()
                .play(effective.url.clone(), effective.stream_headers.clone());
            player.set_play_url(effective);
        }
        HookDecision::Skip { reason } => {
            mineral_log::info!(
                target: "script",
                song_id = song.id.as_str(),
                reason,
                "before_play 跳过本曲"
            );
            player.notify().toast(
                mineral_protocol::ToastKind::Warn,
                format!("脚本跳过播放:{reason}"),
            );
            player.next_song();
        }
    }
}

/// 原播放路径:capture 起播 + 回填 `play_url`(放行 / 无脚本共用)。
fn start_play(player: &PlayerCore, song: &Song, pu: PlayUrl) {
    download::play_capturing(player, song, &pu, player.playback_quality());
    player.set_play_url(pu);
}

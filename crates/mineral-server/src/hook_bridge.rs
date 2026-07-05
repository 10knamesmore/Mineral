//! server 侧脚本拦截桥:`before_stream` / `before_download` 的唯一插桩面。
//!
//! 职责边界:把播放 / 下载链路的拦截窗口接到脚本线程
//! ([`ScriptSender::intercept`]),并把裁决落回执行面。无脚本线程时
//! 走完全同步的原路径(零行为变化);拦截一切异常(超时 / 线程退出 /
//! Lua 错误)都收敛为放行。
//!
//! `before_stream` 是**一个决策钩子在两个提交点各 fire 一次**:每首歌走向
//! 「开播」只经一个提交点——即时起播([`before_stream`],预算 =
//! `hook_timeout_ms`)或 gapless 预取武装([`on_prefetch_ready`],预算 =
//! 预取窗口,裁决在关键路径外等)。取链失败走 unplayable 口
//! ([`on_unplayable_current`] / [`on_unplayable_prefetch`],ctx 无原始 URL),
//! 脚本可改写顶入可播流。
//!
//! 已知取舍:本地命中(`resolve_local`)与 gapless **边界**(`Adopt`,那时已
//! 无缝在播,改写会 blip)不过 hook——前者改写语义不成立(用户自己的文件),
//! 后者没有改写窗口;预取武装前的窗口已由 [`on_prefetch_ready`] 覆盖。

use std::time::Duration;

use mineral_model::{BitRate, PlayUrl, Song, SongId, StreamLayout};
use mineral_script::{
    BeforeDownloadCtx, BeforeStreamCtx, HookDecision, HookMode, RewriteSpec, ScriptSender,
};

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
            .intercept_download(
                BeforeDownloadCtx::new(song.clone(), Some(original.clone())),
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

/// prefetch 口味的拦截预算:预取窗口减 2s 余量(留给裁决后的取链 / 武装),
/// 但不低于 immediate 档(窗口配得极小时退化为同款预算)。
fn prefetch_budget(player: &PlayerCore, immediate: Duration) -> Duration {
    Duration::from_millis(player.gapless_prefetch_ms().saturating_sub(2_000)).max(immediate)
}

/// `before_stream` 即时提交点:当前歌远端 URL 就绪后、起播前。
///
/// 无脚本线程 → 同步直走原路径(play_capturing + 回填 play_url),与
/// 插桩前逐指令一致;有脚本 → spawn 异步拦截,裁决回来再起播(拦截
/// 窗口内的静音由软超时封顶)。
///
/// # Params:
///   - `player`: 播放核心
///   - `song`: 即将起播的歌
///   - `pu`: 解析出的播放 URL(本函数消费)
pub(crate) fn before_stream(player: &PlayerCore, song: &Song, pu: PlayUrl) {
    let gate = player.hook_gate();
    let Some(sender) = gate.active().cloned() else {
        start_play(player, song, pu);
        return;
    };
    let player = player.clone();
    let song = song.clone();
    tokio::spawn(async move {
        let ctx = BeforeStreamCtx::new(song.clone(), HookMode::Immediate, Some(pu.clone()));
        let decision = sender.intercept_stream(ctx, gate.timeout).await;
        apply_play_decision(&player, &song, pu, decision);
    });
}

/// `before_stream` 即时提交点的 unplayable 口:当前歌取链失败(无可播 URL)。
///
/// 无脚本 → 维持原失败语义([`finish_failed`]);有脚本 → 拦截,`Rewrite`(须给
/// url)= 顶入可播流,`Continue`/空改写 = 原失败语义,`Skip` = 推进下一首。
pub(crate) fn on_unplayable_current(player: &PlayerCore, song: &Song) {
    let gate = player.hook_gate();
    let Some(sender) = gate.active().cloned() else {
        finish_failed(player, song);
        return;
    };
    let player = player.clone();
    let song = song.clone();
    tokio::spawn(async move {
        let ctx = BeforeStreamCtx::new(song.clone(), HookMode::Immediate, /*original*/ None);
        let decision = sender.intercept_stream(ctx, gate.timeout).await;
        if !still_current(&player, &song.id) {
            mineral_log::debug!(
                target: "script",
                song_id = song.id.as_str(),
                "拦截窗口内已切歌,丢弃 unplayable 裁决"
            );
            return;
        }
        match decision {
            HookDecision::Continue => finish_failed(&player, &song),
            HookDecision::Rewrite(spec) => {
                match effective_play_url(&song.id, /*original*/ None, &spec) {
                    Some(effective) => {
                        mineral_log::info!(
                            target: "script",
                            song_id = song.id.as_str(),
                            url = %effective.url,
                            "before_stream 改写顶入播放 URL"
                        );
                        play_rewritten(&player, effective);
                    }
                    // 改写没给 url = 补救不成立,按原失败语义收场。
                    None => finish_failed(&player, &song),
                }
            }
            HookDecision::Skip { reason } => {
                mineral_log::info!(
                    target: "script",
                    song_id = song.id.as_str(),
                    reason,
                    "before_stream 跳过不可播曲"
                );
                player.notify().toast(
                    mineral_protocol::ToastKind::Warn,
                    format!("脚本跳过播放:{reason}"),
                );
                player.next_song();
            }
        }
    });
}

/// `before_stream` 预取提交点:gapless 预取的下一首 URL 就绪后、武装进引擎 next 槽前。
///
/// 无脚本 → 直走原武装路径(零行为变化);有脚本 → 拦截(预算 = 预取窗口,音乐照常
/// 播),裁决回来过守卫(预取未被切歌 / 改队列作废)再落地:`Continue` 武装原 URL、
/// `Rewrite` 武装改写流(不 capture)、`Skip` 否决预排(队列不动,`check_prefetch`
/// 下个 tick 越过它重排)。
pub(crate) fn on_prefetch_ready(player: &PlayerCore, song_id: &SongId, play_url: PlayUrl) {
    let gate = player.hook_gate();
    let Some(sender) = gate.active().cloned() else {
        crate::gapless::on_prefetch_url_ready(player, song_id, play_url);
        return;
    };
    // ctx 要歌的快照;队列里已找不到 = 预取作废,丢。
    let Some(song) = find_in_queue(player, song_id) else {
        return;
    };
    let player = player.clone();
    let song_id = song_id.clone();
    tokio::spawn(async move {
        let budget = prefetch_budget(&player, gate.timeout);
        let ctx = BeforeStreamCtx::new(song, HookMode::Prefetch, Some(play_url.clone()));
        let decision = sender.intercept_stream(ctx, budget).await;
        if !prefetch_still_pending(&player, &song_id) {
            mineral_log::debug!(
                target: "script",
                song_id = song_id.as_str(),
                "拦截窗口内预取已作废,丢弃裁决"
            );
            return;
        }
        match decision {
            HookDecision::Continue => {
                crate::gapless::on_prefetch_url_ready(&player, &song_id, play_url);
            }
            HookDecision::Rewrite(spec) => {
                if let Some(effective) = effective_play_url(&song_id, Some(play_url), &spec) {
                    mineral_log::info!(
                        target: "script",
                        song_id = song_id.as_str(),
                        url = %effective.url,
                        "before_stream 改写预取 URL"
                    );
                    crate::gapless::on_prefetch_rewritten(&player, &song_id, effective);
                }
            }
            HookDecision::Skip { reason } => veto_next(&player, &song_id, &reason),
        }
    });
}

/// `before_stream` 预取提交点的 unplayable 口:预取的下一首取链失败。
///
/// 无脚本 → 静默(现状:预取失败等边界 `Fallback` 兜底,届时走即时口再问);
/// 有脚本 → `Rewrite` 补出可播流照常武装(无缝保住),`Skip` 否决预排,
/// `Continue` 维持静默兜底。
pub(crate) fn on_unplayable_prefetch(player: &PlayerCore, song_id: &SongId) {
    let gate = player.hook_gate();
    let Some(sender) = gate.active().cloned() else {
        return;
    };
    let Some(song) = find_in_queue(player, song_id) else {
        return;
    };
    let player = player.clone();
    let song_id = song_id.clone();
    tokio::spawn(async move {
        let budget = prefetch_budget(&player, gate.timeout);
        let ctx = BeforeStreamCtx::new(song, HookMode::Prefetch, /*original*/ None);
        let decision = sender.intercept_stream(ctx, budget).await;
        if !prefetch_still_pending(&player, &song_id) {
            return;
        }
        match decision {
            HookDecision::Continue => {}
            HookDecision::Rewrite(spec) => {
                if let Some(effective) = effective_play_url(&song_id, /*original*/ None, &spec) {
                    mineral_log::info!(
                        target: "script",
                        song_id = song_id.as_str(),
                        url = %effective.url,
                        "before_stream 改写武装预取 URL"
                    );
                    crate::gapless::on_prefetch_rewritten(&player, &song_id, effective);
                }
            }
            HookDecision::Skip { reason } => veto_next(&player, &song_id, &reason),
        }
    });
}

/// 当前歌守卫:拦截窗口内可能已切歌——不再是当前曲就整体丢弃裁决
/// (切歌路径早已 stop 音频,这里再动作反而会复活一首旧歌)。
fn still_current(player: &PlayerCore, song_id: &SongId) -> bool {
    player.with_state(|st| {
        st.current_song
            .as_ref()
            .is_some_and(|current| current.id == *song_id)
    })
}

/// 预取守卫:这首仍是「已发起预拉、尚未武装」的下一曲才落地裁决——切歌 / 改队列
/// 会清预取簿记(`invalidate_prefetch`),届时裁决自然作废。
fn prefetch_still_pending(player: &PlayerCore, song_id: &SongId) -> bool {
    player.with_state(|st| st.prefetch_fired_for.as_ref() == Some(song_id) && st.queued.is_none())
}

/// 队列内按 id 取歌的快照(拦截 ctx 用);不在队列返回 `None`。
fn find_in_queue(player: &PlayerCore, song_id: &SongId) -> Option<Song> {
    player.with_state(|st| st.queue.iter().find(|s| s.id == *song_id).cloned())
}

/// 预取否决:把这首的**下标**记入本窗口否决集(队列不动),并复位预拉标记让
/// `check_prefetch` 下个 tick 按否决后的预测重排(通常落到再下一首,无缝保住)。
///
/// 重新定位下标而不缓存 fire 时的值:裁决窗口内队列可能已变——变了则守卫/此处
/// 的身份核对会让本裁决自然作废,不存在陈旧下标。
fn veto_next(player: &PlayerCore, song_id: &SongId, reason: &str) {
    player.with_state(|st| {
        if let Some(idx) = crate::queue::next_index(st)
            && st.queue.get(idx).is_some_and(|s| s.id == *song_id)
        {
            st.prefetch_vetoed.push(idx);
        }
        st.prefetch_fired_for = None;
    });
    mineral_log::info!(
        target: "script",
        song_id = song_id.as_str(),
        reason,
        "before_stream 否决预取下一首"
    );
    player.notify().toast(
        mineral_protocol::ToastKind::Warn,
        format!("脚本跳过下一首:{reason}"),
    );
}

/// 把改写意图落成可播的 effective [`PlayUrl`]。
///
/// 有原始 URL 时在其上覆写(语义与历史一致:改了 url 才动 layout,默认流式打开);
/// 无原始 URL(unplayable)时改写**必须给 url**,其余元信息脚本给多少用多少
/// (码率/格式可透传给展示层),没给的取安全默认(码率/大小 0、格式未知)。
///
/// # Params:
///   - `song_id`: 目标歌 id(unplayable 场景装配新 PlayUrl 用)
///   - `original`: 原始 URL;`None` = 无可播 URL
///   - `spec`: 脚本的改写意图
///
/// # Return:
///   可播的 effective;unplayable 且改写没给 url 时 `None`(补救不成立)。
fn effective_play_url(
    song_id: &SongId,
    original: Option<PlayUrl>,
    spec: &RewriteSpec,
) -> Option<PlayUrl> {
    let Some(mut effective) = original else {
        return Some(PlayUrl {
            song_id: song_id.clone(),
            url: spec.new_url()?.clone(),
            bitrate_bps: spec.bitrate_bps().unwrap_or(0),
            quality: spec.new_quality().unwrap_or(BitRate::Standard),
            size: 0,
            format: spec.format().cloned().unwrap_or_default(),
            bit_depth: None,
            stream_headers: spec
                .stream_headers()
                .map(<[(String, String)]>::to_vec)
                .unwrap_or_default(),
            layout: spec.layout().unwrap_or(StreamLayout::Chunked),
            // unplayable 补救必然顶入外源流:歌词等借自原源的元数据从此不可尽信。
            substituted: true,
        });
    };
    if let Some(url) = spec.new_url() {
        effective.url = url.clone();
        // 顶换了 URL → 目标容器可能与原曲不同:脚本显式给 layout 则用之,否则默认流式打开
        // (不预扫,永不因分片容器全扫而起播卡顿;代价仅流式期间不支持向后 seek)。
        effective.layout = spec
            .layout()
            .unwrap_or(mineral_model::StreamLayout::Chunked);
        effective.substituted = true;
    }
    if let Some(quality) = spec.new_quality() {
        effective.quality = quality;
    }
    // 顶换流的实测元信息(纯展示):脚本给了就覆盖,原曲的码率/格式对替身流没有意义。
    if let Some(bitrate_bps) = spec.bitrate_bps() {
        effective.bitrate_bps = bitrate_bps;
    }
    if let Some(format) = spec.format() {
        effective.format = format.clone();
    }
    // 改写顶替进来的 url 同步带上其取流头(如 B站 baseUrl 需 `Referer`),否则播放 403。
    if let Some(headers) = spec.stream_headers() {
        effective.stream_headers = headers.to_vec();
    }
    Some(effective)
}

/// 把即时提交点的裁决落到播放执行面。
fn apply_play_decision(
    player: &PlayerCore,
    song: &Song,
    original: PlayUrl,
    decision: HookDecision,
) {
    if !still_current(player, &song.id) {
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
            let Some(effective) = effective_play_url(&song.id, Some(original), &spec) else {
                return;
            };
            mineral_log::info!(
                target: "script",
                song_id = song.id.as_str(),
                url = %effective.url,
                "before_stream 改写播放 URL"
            );
            play_rewritten(player, effective);
        }
        HookDecision::Skip { reason } => {
            mineral_log::info!(
                target: "script",
                song_id = song.id.as_str(),
                reason,
                "before_stream 跳过本曲"
            );
            player.notify().toast(
                mineral_protocol::ToastKind::Warn,
                format!("脚本跳过播放:{reason}"),
            );
            player.next_song();
        }
    }
}

/// 起播一条改写过的流:**不 capture 入缓存**——缓存按 song_id+quality 入键,
/// 改写内容与原曲是否一致由脚本自负,污染缓存代价高;改写流每次现拉。
fn play_rewritten(player: &PlayerCore, effective: PlayUrl) {
    player.audio().play(
        effective.url.clone(),
        effective.stream_headers.clone(),
        effective.layout,
    );
    player.set_play_url(effective);
}

/// 取链失败的原失败语义:提升为 `track_finished("error")`(脚本 / 订阅 client 可见)。
fn finish_failed(player: &PlayerCore, song: &Song) {
    player
        .notify()
        .track_finished(song, mineral_protocol::FinishReason::Error);
}

/// 原播放路径:capture 起播 + 回填 `play_url`(放行 / 无脚本共用)。
fn start_play(player: &PlayerCore, song: &Song, pu: PlayUrl) {
    download::play_capturing(player, song, &pu, player.playback_quality());
    player.set_play_url(pu);
}

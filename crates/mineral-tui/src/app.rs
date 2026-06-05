//! 顶层 [`App`] 状态与同步主事件循环。
//!
//! **4c 重构后**:player 业务(submit_play_song / next/prev/cycle_mode / queue 管理 /
//! auto-next / prefetch)整体搬到 server (`mineral_server::PlayerCore`)。App 退化
//! 为「转发用户意图 + 渲染 server 状态镜像」。每帧 tick 做一次版本门控同步
//! (PlayerSync,报已持版本、只收落后的重段)灌进 AppState 镜像;按键直接转
//! `client.play_song / cycle_play_mode / ...` 等高级意图。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use mineral_model::Song;
use mineral_protocol::{DownloadTarget, PlayerSync};
use mineral_server::Client;
use mineral_task::TaskEvent;
use ratatui::layout::Position;
use ratatui_image::picker::Picker;

use crate::components::popup::{OverlayAction, OverlayKind, OverlayResponse, OverlayStack};
use crate::components::toast::download_toast::DownloadNotifier;
use crate::components::toast::notifications::Notifications;
use crate::render::anim::{Transition, ticks16_from_ms};
use crate::render::theme::Theme;
use crate::runtime::action::{Action, SeekDelta, SelectionMove, VolumeDelta};
use crate::runtime::cover_encode::CoverEncoder;
use crate::runtime::cover_fetch::CoverFetcher;
use crate::runtime::keymap::{Keymap, chord_from_event};
use crate::runtime::state::{AppState, View};
use crate::tui::Tui;
use crate::view::draw;

/// 应用顶层状态。
pub struct App {
    /// 是否退出主循环。
    pub should_quit: bool,

    /// 当前主题(`Arc` 共享:future 热重载时整体换,渲染处只读引用)。
    pub theme: std::sync::Arc<Theme>,

    /// 业务状态(视图、选中、playback 镜像、加载缓存等)。
    pub state: AppState,

    /// 键 → 动作绑定表(由配置 keys/behavior 段落地)。
    keymap: Keymap,

    /// 浮层栈(queue / confirm / disconnect):统一托管开关、光标、弹出动画。
    pub(crate) overlays: OverlayStack,

    /// 整屏转场动画:`None` 为正常运行;`Some` 且 `leaving()` 为退出收缩(归零后真正退出),
    /// `Some` 且非 `leaving()` 为启动扩大(推满后转入正常运行)。两者时间上互斥,共用此字段。
    /// 退出仅正常退出(confirm)触发;Ctrl-C / 断连立即退,不走它。
    pub(crate) transition: Option<Transition>,

    /// 上一次 tick 时间。
    pub last_tick: Instant,

    /// 主循环帧间隔(配置 `animation.frame_tick_ms`,默认 ≈60fps)。
    frame_tick: Duration,

    /// 整屏转场动画时长(tick,由配置 `animation.transition_ms` 按帧率折算);
    /// 扩大与收缩同速对称。
    transition_ticks: u16,

    /// client 侧心跳间隔(配置 `daemon.heartbeat_secs`)。
    heartbeat: Duration,

    /// Server client:所有「调命令 / 拉 snapshot / 拉事件」都走它。
    /// 实现可能是同进程 `ClientHandle`,也可能是跨进程 `RemoteClient`,通过
    /// [`Client`] trait 抽象。**player 业务在 server 端**;App 只 forward 意图。
    client: Arc<dyn Client>,

    /// Client 端 cover fetcher。封面是 client-local 资源,不归 server 管。
    cover_fetcher: CoverFetcher,

    /// Client 端 cover 编码器:把封面 resize + kitty 编码挪出渲染线程(worker 上跑),
    /// `drain` 回填 `cover_protocols`。与 `cover_fetcher` 互补成完整异步封面管线。
    cover_encoder: CoverEncoder,

    /// topbar 通知层:多条堆叠的提示通道(flash / 常驻进度),与具体业务解耦。
    pub(crate) notifications: Notifications,

    /// 下载 → 通知层的翻译器(持下载专属去重状态);通知层之上的众多使用方之一。
    download_notifier: DownloadNotifier,

    /// 终端图片协议探测结果。
    pub picker: Picker,

    /// 进 alternate screen 前捕获的终端光标位置,作为整屏 expand/collapse 的缩放锚点:
    /// expand 从此点铺开、collapse 收回此点(对得上 `LeaveAlternateScreen` 后光标实际
    /// 回到的行)。无 TTY 时为 `None`,缩放退化回屏幕居中。
    pub(crate) launch_anchor: Option<Position>,
}

impl App {
    /// 构造 App:主题 / 键表 / 各段手感由注入的配置一次性落地。
    ///
    /// # Params:
    ///   - `client`: 跟 server 交互的句柄
    ///   - `cover_fetcher`: client 端 cover fetcher
    ///   - `picker`: 终端图片协议能力
    ///   - `launch_anchor`: 进 alternate screen 前捕获的光标位置,作整屏 expand/collapse
    ///     的缩放锚点;`None`(无 TTY)时缩放退化回屏幕居中
    ///   - `cfg`: 已加载的全局配置(`Arc` 共享只读)
    pub fn new(
        client: Arc<dyn Client>,
        cover_fetcher: CoverFetcher,
        cover_encoder: CoverEncoder,
        picker: Picker,
        launch_anchor: Option<Position>,
        cfg: Arc<mineral_config::Config>,
    ) -> Self {
        let tui_cfg = cfg.tui();
        let theme = Arc::new(Theme::from_config(tui_cfg.theme()));
        let keymap = Keymap::from_config(tui_cfg.keys(), tui_cfg.behavior());
        let anim = tui_cfg.animation();
        let tick_ms = *anim.frame_tick_ms();
        let overlays = OverlayStack::new(ticks16_from_ms(*anim.popup_anim_ms(), tick_ms));
        let notifications = Notifications::new(
            *tui_cfg.toast().flash_ttl_secs(),
            ticks16_from_ms(*anim.toast_anim_ms(), tick_ms),
        );
        let frame_tick = Duration::from_millis(tick_ms);
        let transition_ticks = ticks16_from_ms(*anim.transition_ms(), tick_ms);
        let heartbeat = Duration::from_secs(*cfg.daemon().heartbeat_secs());
        let mut state = AppState::new(cfg);
        // 把渲染处投递编码请求的发送端接到真实 worker(禁用态编码器是无接收端的 sender)。
        state.cover_encode_tx = cover_encoder.sender();
        Self {
            should_quit: false,
            theme,
            state,
            keymap,
            overlays,
            transition: None,
            last_tick: Instant::now(),
            frame_tick,
            transition_ticks,
            heartbeat,
            client,
            cover_fetcher,
            cover_encoder,
            notifications,
            download_notifier: DownloadNotifier::new(),
            picker,
            launch_anchor,
        }
    }

    /// 同步主事件循环:绘制 → 等事件 → 每帧间隔拉数据 + 推进动画/频谱
    /// (节奏由配置 `animation.frame_tick_ms` 决定,默认 ~60fps)。
    pub fn run(&mut self, tui: &mut Tui) -> color_eyre::Result<()> {
        // 启动时同步一次(versions 初始为 0 → 必然全量),in-proc / connect 都立即
        // 看到 server 状态;与 tick 路径同一条 sync 通道,无特殊分支。
        let sync = self.client.player_sync(self.state.versions);
        self.apply_player_sync(sync);

        // 启动扩大转场:界面从中心小框向四周铺满,与退出收缩反向对称。推满后转入正常运行。
        self.transition = Some(Transition::expanding(self.transition_ticks));

        // client 侧心跳(间隔 = daemon.heartbeat_secs):报 server 看不到的 UI / 缓存状态(启动即首条)。
        let mut last_heartbeat = Instant::now();
        self.log_heartbeat();

        // 退出信号 watcher:SIGTERM / SIGINT / SIGHUP 进来时不再 silent kill,而是由
        // 后台 task 记日志 + 置标志,主循环据此走正常退出(`Tui::exit` 还原终端)。
        let shutdown = crate::runtime::signal::spawn_watcher()?;

        while !self.should_quit {
            if shutdown.load(Ordering::Acquire) {
                self.should_quit = true;
                break;
            }
            // daemon 被单独 kill / crash → 链路断开。不僵死在「请求全兜底默认值」的
            // 状态:压入断连提示浮层(记一条 error),进入下面的「显示话术 + 等按键退出」分支。
            if !self.overlays.is_disconnected() && !self.client.connected() {
                mineral_log::error!(target: "tui", "daemon connection lost, awaiting key to exit");
                self.overlays.push(OverlayKind::disconnect());
            }
            if self.overlays.is_disconnected() {
                // 只渲染断连提示 + 推进其弹出动画 + 等按键退出;daemon 没了,正常路径全是
                // 兜底默认值,跳过后端同步。fatal 态直接退出(不走 dispatch,不玩退出收缩动画)。
                // 清掉转场:本分支不推进它,启动即断连否则会把扩大动画卡在空屏。
                self.transition = None;
                tui.draw(|f| draw(f, self))?;
                if event::poll(self.frame_tick)?
                    && let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press
                {
                    self.should_quit = true;
                }
                self.overlays.tick();
                continue;
            }

            tui.draw(|f| draw(f, self))?;

            let timeout = self.frame_tick.saturating_sub(self.last_tick.elapsed());
            if event::poll(timeout)? {
                self.handle_event(&event::read()?);
            }
            if self.last_tick.elapsed() >= self.frame_tick {
                // 转场动画(启动扩大 / 退出收缩)进行中:只推进它 + 重绘(上方 tui.draw),
                // 跳过后端同步;退出转场归零即退,启动转场推满即转入正常运行。
                if self.transition.is_some() {
                    self.tick_transition();
                    self.last_tick = Instant::now();
                    continue;
                }
                self.drain_task_events();
                let snap = self.client.audio_snapshot();
                self.state.playback.apply_audio_snapshot(snap);
                self.update_spectrum();
                self.state.view_pos.tick();
                self.state.fullscreen_pos.tick();
                self.tick_overlays();
                let sync = self.client.player_sync(self.state.versions);
                self.apply_player_sync(sync);
                self.drain_ready_covers();
                self.sync_spectrum_palette();
                self.drain_ready_protocols();
                crate::runtime::prefetch::tick(&mut self.state, &*self.client, &self.cover_fetcher);
                self.state.tasks_snapshot = self.client.task_snapshot();
                self.state.cover_loading = self.state.cover_pending.len();
                // 每帧把下载进度喂进通知层(翻译成常驻进度 / 完成 flash),再推进所有通知动画。
                let dp = self.client.download_progress();
                self.download_notifier.feed(&mut self.notifications, &dp);
                self.notifications.tick();
                self.last_tick = Instant::now();
                if last_heartbeat.elapsed() >= self.heartbeat {
                    self.log_heartbeat();
                    last_heartbeat = Instant::now();
                }
            }
        }
        Ok(())
    }

    /// 推进浮层动画一拍;并处理「全屏下居中浮层刚被移除」的封面残影。
    ///
    /// 居中浮层(如 quit 确认)在全屏时会压住左侧封面的中段。kitty 协议把整行 unicode
    /// 占位符打包在该行**最左 cell**、其余 cell `set_skip(true)`,而 ratatui 的 buffer diff
    /// 跳过未变 cell —— 浮层只盖了封面中段、没碰最左驱动 cell,关闭后那几行不会自行重发,
    /// 中段残留浮层底色(残影)。故在该居中浮层(退场动画放完)真正出栈的那一拍,清一次封面
    /// 协议缓存:下一帧 `cover_image` 按需重建、重新 transmit + 全量 re-place,残影消除。
    ///
    /// **仅对居中浮层做此事**:停靠浮层(queue 贴右)不压封面,清它纯属白白触发封面重新解码
    /// / base64 编码(几十毫秒、卡掉一帧),故停靠浮层出栈不刷新。
    fn tick_overlays(&mut self) {
        let before = self.overlays.len();
        let closing_centered = self.overlays.any_leaving_centered();
        self.overlays.tick();
        if self.state.fullscreen && closing_centered && self.overlays.len() < before {
            self.state.cover_protocols.borrow_mut().clear();
        }
    }

    /// 推进整屏转场动画一帧。`settled()`(进度抵达目标)时收尾:退出转场(`leaving()`)置
    /// `should_quit`,启动转场转入正常运行;两者随后统一清空 `transition`。无转场时为空操作。
    fn tick_transition(&mut self) {
        let Some(anim) = &mut self.transition else {
            return;
        };
        anim.tick();
        if anim.settled() {
            if anim.leaving() {
                self.should_quit = true;
            }
            self.transition = None;
        }
    }

    /// client 侧心跳:把 server 看不到的 UI / 缓存状态打一条 info。大缓存
    /// (tracks / cover / lyrics)都在 client 端,server 心跳报不了,这里补上。
    fn log_heartbeat(&self) {
        let s = &self.state;
        let liked = s
            .liked_ids
            .values()
            .fold(0_usize, |acc, set| acc + set.len());
        mineral_log::info!(
            target: "heartbeat",
            view = ?s.view,
            playlists = s.playlists.len(),
            tracks_cached = s.tracks_cache.len(),
            tracks_requested = s.tracks_requested.len(),
            lyrics_cached = s.lyrics_cache.len(),
            covers_cached = s.cover_cache.len(),
            covers_pending = s.cover_pending.len(),
            liked,
            queue_len = s.queue.len(),
            "client status"
        );
    }

    /// 把 server 的版本门控同步灌进 AppState 镜像。每 `TICK` 调一次。
    ///
    /// 核心语义:**重段缺席 ≠ 清空**——`None` 表示「与已有版本一致」,镜像原地保持;
    /// 只有 `Some` 才整体替换。轻段(play_mode / play_origin)每 tick 照常灌。
    fn apply_player_sync(&mut self, sync: PlayerSync) {
        self.state.versions = sync.versions;
        self.state.playback.play_origin = sync.play_origin;
        self.state.playback.mode = sync.play_mode;
        if let Some(q) = sync.queue {
            self.state.queue = q.queue;
            self.state.original_queue = q.original_queue;
            // 不灌 sync.queue_sel —— 那是 server 的「在播位置锚点」(prev/next 用),语义
            // 不同于 UI 光标;在播歌已由 ▶ 标记单独表达。queue 浮层光标是纯客户端态,
            // 只钳防越界。
            self.overlays.clamp_queue(self.state.queue.len());
        }
        if let Some(c) = sync.current {
            self.state.current = c.current_song.clone();
            self.state.playback.track = c.current_song;
            self.state.playback.play_url = c.play_url;
            // lyrics cache: 仅按 server 给的「current_lyrics_song_id」灌。歌词在 channel
            // 层已结构化清洗,这里直接收下整份(原文 / 逐字 / 翻译 / 罗马音),不再解析。
            if let (Some(song_id), Some(lyrics)) = (c.current_lyrics_song_id, c.current_lyrics)
                && !self.state.lyrics_cache.contains_key(&song_id)
            {
                self.state.lyrics_cache.insert(song_id, lyrics);
            }
        }
    }

    /// 把 cover_fetcher 就绪的图写进 cache + 色板写进 `cover_palettes` + 清掉对应 protocol
    /// (下次渲染重建)。取色失败(`palette = None`)只是不落色板,图照常缓存显示。
    fn drain_ready_covers(&mut self) {
        for ready in self.cover_fetcher.drain_ready() {
            self.state.cover_pending.remove(&ready.url);
            if let Some(palette) = ready.palette {
                self.state.cover_palettes.insert(ready.url.clone(), palette);
            }
            self.state
                .cover_cache
                .insert(ready.url.clone(), ready.image);
            self.state.cover_protocols.borrow_mut().remove(&ready.url);
        }
    }

    /// 协调当前播放封面的频谱配色:新封面取色就绪则从**当前可见配色**缓动过去,否则保持现状。
    ///
    /// 身份判定(`cover_url` 变化、色带是否就绪)全在此(app 层);频谱只收
    /// `begin_cover_transition` / `clear_cover` 两个命令,不持有歌曲 / URL 身份。
    ///
    /// - 当前封面与 `spectrum_cover` 一致 → 不动。
    /// - 当前封面变了 + 色带已就绪 → `begin_cover_transition`(从上一张封面 / hue 起步),记下 key。
    /// - 当前封面变了 + 图已到但**取色失败**(在 `cover_cache` 却不在 `cover_palettes`)→ 回退 hue,标记已处理。
    /// - 当前封面变了 + 图还在抓 → **保持当前可见态**(上一张封面继续显示),下个 tick 再看。
    ///   这是"红专辑换蓝专辑 → 红→蓝"的关键:抓图途中不回退 hue,等蓝就绪直接红→蓝。
    /// - 无当前歌 / 无封面 → 回退 hue。
    fn sync_spectrum_palette(&mut self) {
        let cur = self
            .state
            .current
            .as_ref()
            .and_then(|s| s.cover_url.clone());
        let Some(url) = cur else {
            if self.state.spectrum_cover.is_some() {
                self.state.spectrum.clear_cover();
                self.state.spectrum_cover = None;
            }
            return;
        };
        if self.state.spectrum_cover.as_ref() == Some(&url) {
            return;
        }
        if let Some(palette) = self.state.cover_palettes.get(&url).cloned() {
            self.state
                .spectrum
                .begin_cover_transition(palette, &self.theme);
            self.state.spectrum_cover = Some(url);
        } else if self.state.cover_cache.contains_key(&url) {
            // 图已回但无色板 = 取色失败:回退 hue,标记已处理(不再每帧重试)。
            self.state.spectrum.clear_cover();
            self.state.spectrum_cover = Some(url);
        }
        // else:封面还在抓,保持当前可见态(上一张封面 / hue)不动,等就绪后再红→蓝。
    }

    /// 把编码 worker 就绪的封面协议装回 `cover_protocols`,并出 `cover_encode_pending`。
    /// 之后帧渲染该封面即命中已编码协议、直接 place,不再在渲染线程上 resize / 编码。
    fn drain_ready_protocols(&mut self) {
        for r in self.cover_encoder.drain_ready() {
            self.state
                .cover_encode_pending
                .borrow_mut()
                .remove(&(r.url.clone(), r.dims));
            self.state
                .cover_protocols
                .borrow_mut()
                .insert(r.url, (r.protocol, r.dims));
        }
    }

    /// 把 client.pull_pcm 拿到的样本喂给 fft computer。in-proc 和 connect 走同一路径。
    fn update_spectrum(&mut self) {
        // 每 tick 最多拉一个 FFT 窗的样本:正常一帧只来几百样本,卡顿后一帧即可补满整窗。
        let pop_chunk = self.state.fft.window_size();
        let (samples, sample_rate) = self.client.pull_pcm(pop_chunk);
        if !samples.is_empty() {
            self.state.fft.push(&samples);
        }
        let target_bars = self.state.spectrum.target_bars.get();
        let bars = self.state.fft.compute(sample_rate, target_bars);
        self.state.spectrum.tick(
            self.state.playback.playing,
            self.state.playback.volume_pct,
            bars.as_deref(),
        );
    }

    /// 把 server 端积攒的 task events 拉过来 apply 到 [`AppState`]。
    fn drain_task_events(&mut self) {
        let events = self.client.drain_task_events();
        for ev in &events {
            // Notice 是给状态栏看的瞬时提示(如下载进度),不进 AppState 数据;其余照常 apply。
            match ev {
                TaskEvent::Notice { text } => {
                    self.notifications.flash_text(text.clone());
                }
                _ => self.state.apply(ev),
            }
        }
    }

    /// 处理一个 crossterm 事件;目前只关心 KeyEvent 的按下边沿。
    fn handle_event(&mut self, ev: &Event) {
        if let Event::Key(key) = ev
            && key.kind == KeyEventKind::Press
        {
            self.handle_key(key);
        }
    }

    /// 顶层按键分发:Ctrl-C 永远退出;活跃浮层优先吃键,否则走全局 / 主视图。
    fn handle_key(&mut self, key: &KeyEvent) {
        // Ctrl-C 强制退出(skip 一切)。
        if matches!(
            (key.modifiers, key.code),
            (KeyModifiers::CONTROL, KeyCode::Char('c'))
        ) {
            self.should_quit = true;
            return;
        }

        // 整屏转场动画进行中(启动扩大 / 退出收缩):吞掉所有其他按键(动画不可打断,
        // Ctrl-C 已在上面强退)。
        if self.transition.is_some() {
            return;
        }

        // 查一次表,全程复用:浮层在顶时导航族动作经 on_action 进浮层(跟随键位重映射
        // 与 behavior 步长),浮层不认或未命中再走浮层裸键;无浮层走全局 dispatch。
        let action = chord_from_event(key).and_then(|c| self.keymap.lookup(c));

        // 活跃浮层(栈顶未退场)优先吃键。Consumed 吞掉、Pass 半穿透给全局、Do 交意图执行。
        if let Some(resp) = self.overlays.dispatch_key(key, action, &self.state) {
            match resp {
                OverlayResponse::Consumed => {}
                OverlayResponse::Pass => self.handle_overlay_passthrough(key),
                OverlayResponse::Do(overlay_action) => self.run_overlay_action(overlay_action),
            }
            return;
        }

        // —— 以下:无活跃浮层 ——
        if self.state.search_mode {
            self.handle_search_key(key);
            return;
        }

        // dispatch 执行查表意图。上下文裁决(全屏屏蔽列表导航 / 搜索 `/`)在各
        // 执行器开头判,保证「键 → 行为」中段可被 config 表替换而闸语义不动。
        if let Some(action) = action {
            self.dispatch(action);
        }
    }

    /// 执行一个查表命中的 [`Action`]:每分支一行调用对应执行器,逻辑都在执行器里。
    fn dispatch(&mut self, action: Action) {
        match action {
            Action::ToggleFullscreen => self.toggle_fullscreen(),
            Action::OpenQueue => self.open_queue(),
            Action::OpenQuitConfirm => self.overlays.push(OverlayKind::confirm()),
            Action::CycleLyricExtra => self.state.cycle_lyric_extra(),
            Action::EnterSearch => self.enter_search(),
            Action::MoveSelection(mv) => self.move_selection(mv),
            Action::ActivateSelection => self.activate_selection(),
            Action::BackOrClearSearch => self.back_or_clear_search(),
            Action::TogglePlayPause => self.toggle_play_pause(),
            Action::CyclePlayMode => self.client.cycle_play_mode(),
            Action::NudgeVolume(VolumeDelta(delta)) => self.nudge_volume(delta),
            Action::SeekRelative(SeekDelta(secs)) => self.seek_relative(secs),
            Action::PrevOrRestart => self.client.prev_or_restart(),
            Action::NextSong => self.client.next_song(),
            Action::ToggleLoveSelection => self.toggle_love_selection(),
            Action::DownloadSelection => self.download_selection(),
        }
    }

    /// 切换全屏播放态:翻转 `fullscreen` 标志并驱动形变进退场(`eased_in_out`,可中途反向)。
    fn toggle_fullscreen(&mut self) {
        self.state.fullscreen = !self.state.fullscreen;
        if self.state.fullscreen {
            self.state.fullscreen_pos.enter();
        } else {
            self.state.fullscreen_pos.leave();
        }
    }

    /// 执行浮层产生的意图(浮层自身不持有 App,按键产出意图回这里执行)。
    fn run_overlay_action(&mut self, action: OverlayAction) {
        match action {
            // 正常退出:不立即退,而是启动「边框向中心收缩」退场动画,归零后主循环再 break。
            OverlayAction::Quit => {
                self.transition = Some(Transition::collapsing(self.transition_ticks));
            }
            OverlayAction::CloseTop => self.overlays.close_top(),
            OverlayAction::PlayQueueIndex(i) => {
                if let Some(song) = self.state.queue.get(i).cloned() {
                    self.client.play_song(song);
                }
            }
        }
    }

    /// 浮层放行(半穿透)的按键:queue 打开时仍可切歌词 / 控播放;其余动作忽略。
    fn handle_overlay_passthrough(&mut self, key: &KeyEvent) {
        let Some(action) = chord_from_event(key).and_then(|c| self.keymap.lookup(c)) else {
            return;
        };
        if Self::passes_overlay(action) {
            self.dispatch(action);
        }
    }

    /// 半穿透白名单:歌词切换 + 播放控制族;列表 / 视图 / 浮层动作不穿透。
    fn passes_overlay(action: Action) -> bool {
        matches!(
            action,
            Action::CycleLyricExtra
                | Action::TogglePlayPause
                | Action::CyclePlayMode
                | Action::NudgeVolume(_)
                | Action::SeekRelative(_)
                | Action::PrevOrRestart
                | Action::NextSong
        )
    }

    /// 打开浮动播放队列,光标定位到在播歌(无在播落 0)。
    fn open_queue(&mut self) {
        let sel = self.state.queue_current_index().unwrap_or(0);
        self.overlays.push(OverlayKind::queue(sel));
    }

    /// 进入搜索输入态并清词;全屏态屏蔽(屏上无列表可滤)。
    fn enter_search(&mut self) {
        if self.state.fullscreen {
            return;
        }
        self.state.search_mode = true;
        self.state.search_q.clear();
    }

    /// 搜索词每次变化后,把当前 view 的 sel 拉回 0。
    fn reset_sel_for_search(&mut self) {
        match self.state.view {
            View::Playlists => self.state.sel_playlist = 0,
            View::Library => self.state.sel_track = 0,
        }
    }

    /// 搜索输入态按键:Esc 退出 + 清词,Enter 退出保留词,Backspace 删字符 / 空词上退出(vim
    /// 命令行行为),字符追加词;改词后复位 sel。
    fn handle_search_key(&mut self, key: &KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.state.search_mode = false;
                self.state.search_q.clear();
            }
            KeyCode::Enter => {
                self.state.search_mode = false;
            }
            KeyCode::Backspace => {
                // vim 行为:query 已空时再删一次 = 退出搜索(等价 Esc)。
                if self.state.search_q.is_empty() {
                    self.state.search_mode = false;
                    return;
                }
                self.state.search_q.pop();
                self.reset_sel_for_search();
                self.state.last_sel_change = Instant::now();
            }
            KeyCode::Char(c) => {
                self.state.search_q.push(c);
                self.reset_sel_for_search();
                self.state.last_sel_change = Instant::now();
            }
            _ => {}
        }
    }

    /// 空格键:有当前曲目时在 pause/resume 间切换;没歌时无动作。
    fn toggle_play_pause(&mut self) {
        if self.state.playback.track.is_none() {
            return;
        }
        if self.state.playback.playing {
            self.client.pause();
        } else {
            self.client.resume();
        }
    }

    /// 在当前音量上加/减 `delta`,clamp 到 0..=100,本地立即更新避免 UI 滞后。
    fn nudge_volume(&mut self, delta: i16) {
        let cur = i16::from(self.state.playback.volume_pct);
        let new = cur.saturating_add(delta).clamp(0, 100);
        let pct = u8::try_from(new).unwrap_or(self.state.playback.volume_pct);
        self.client.set_volume(pct);
        self.state.playback.volume_pct = pct;
    }

    /// 相对当前位置跳 `delta_s` 秒,clamp 到 [0, duration]。
    fn seek_relative(&mut self, delta_s: i64) {
        let dur_ms = self.state.playback.duration_ms();
        if dur_ms == 0 {
            return;
        }
        let cur = i64::try_from(self.state.playback.position_ms).unwrap_or(0);
        let max = i64::try_from(dur_ms).unwrap_or(0);
        let new_ms = cur
            .saturating_add(delta_s.saturating_mul(1000))
            .clamp(0, max);
        let new_u = u64::try_from(new_ms).unwrap_or(0);
        self.client.seek(new_u);
    }

    /// 列表光标移动,按当前 view 落到 `sel_playlist` / `sel_track`,越界钳首末行;
    /// 全屏态屏蔽(屏上无列表)。
    fn move_selection(&mut self, mv: SelectionMove) {
        if self.state.fullscreen {
            return;
        }
        self.state.last_sel_change = Instant::now();
        let max = match self.state.view {
            View::Playlists => self.state.filtered_playlists().len().saturating_sub(1),
            View::Library => self.state.filtered_tracks().len().saturating_sub(1),
        };
        let sel = match self.state.view {
            View::Playlists => &mut self.state.sel_playlist,
            View::Library => &mut self.state.sel_track,
        };
        *sel = match mv {
            SelectionMove::Down(n) => sel.saturating_add(n).min(max),
            SelectionMove::Up(n) => sel.saturating_sub(n),
            SelectionMove::First => 0,
            SelectionMove::Last => max,
        };
    }

    /// 在当前视图「进入」:Playlists 进选中歌单的 Library;Library 设 queue 并播放选中曲。
    /// 全屏态屏蔽。
    fn activate_selection(&mut self) {
        if self.state.fullscreen {
            return;
        }
        self.state.last_sel_change = Instant::now();
        match self.state.view {
            View::Playlists => {
                if let Some(target_id) = self
                    .state
                    .filtered_playlists()
                    .get(self.state.sel_playlist)
                    .map(|p| p.data.id.clone())
                {
                    self.state.search_q.clear();
                    if let Some(raw_idx) = self
                        .state
                        .playlists
                        .iter()
                        .position(|p| p.data.id == target_id)
                    {
                        self.state.sel_playlist = raw_idx;
                    }
                }
                self.state.view = View::Library;
                self.state.view_pos.enter();
                self.state.sel_track = 0;
            }
            View::Library => {
                let filtered = self.state.filtered_tracks();
                let Some(song) = filtered.get(self.state.sel_track).map(|sv| sv.data.clone())
                else {
                    return;
                };
                let new_queue: Vec<Song> = self
                    .state
                    .current_tracks()
                    .into_iter()
                    .map(|sv| sv.data)
                    .collect();
                // Server 端按 PlayMode 决定要不要洗牌;client 只发原始 queue + target。
                self.client.set_queue(new_queue, song.id.clone());
                self.client.play_song(song);
            }
        }
    }

    /// 在当前视图「返回」:搜索非空先清词(选中复位),否则 Library 回 Playlists、
    /// Playlists 无处可回即无操作。全屏态屏蔽。
    fn back_or_clear_search(&mut self) {
        if self.state.fullscreen {
            return;
        }
        self.state.last_sel_change = Instant::now();
        if !self.state.search_q.is_empty() {
            self.state.search_q.clear();
            self.reset_sel_for_search();
            return;
        }
        if matches!(self.state.view, View::Library) {
            self.state.view = View::Playlists;
            self.state.view_pos.leave();
        }
    }

    /// 切换选中曲的 ♥:转发持久化意图 + 本地乐观翻转。仅 Library 有曲可选;全屏态屏蔽。
    fn toggle_love_selection(&mut self) {
        if self.state.fullscreen || !matches!(self.state.view, View::Library) {
            return;
        }
        let filtered = self.state.filtered_tracks();
        if let Some(song) = filtered.get(self.state.sel_track).map(|sv| sv.data.clone()) {
            // 触发持久化(daemon 写本地 + 远端);in-proc fire-and-forget。
            self.client.toggle_love(song.id.clone());
            // 乐观翻转:♥ 立即变,不等 server 确认。
            self.state.toggle_loved_local(&song);
        }
    }

    /// 下载当前视图选中项:Playlists 整张歌单 / Library 单曲。全屏态屏蔽。
    fn download_selection(&mut self) {
        if self.state.fullscreen {
            return;
        }
        match self.state.view {
            View::Playlists => {
                let id = self
                    .state
                    .filtered_playlists()
                    .get(self.state.sel_playlist)
                    .map(|p| p.data.id.clone());
                if let Some(id) = id {
                    self.client.download(DownloadTarget::Playlist(id));
                }
            }
            View::Library => {
                let song = self
                    .state
                    .filtered_tracks()
                    .get(self.state.sel_track)
                    .map(|sv| sv.data.clone());
                if let Some(song) = song {
                    self.client.download(DownloadTarget::Song(Box::new(song)));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use mineral_protocol::{PlayerSync, QueueSync};

    use mineral_model::{MediaUrl, SourceKind};

    use super::App;

    /// 测试对照值 = default.lua 的 `animation.transition_ms`(288)÷ `frame_tick_ms`(16)。
    const TRANSITION_TICKS: u16 = 18;
    use crate::render::anim::Transition;
    use crate::render::palette::{CoverPalette, Rgb};
    use crate::test_support::{app_with_library, app_with_queue, endserenading};

    /// 喂一个 Press 键给 App(走真实事件入口 `handle_event`)。
    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::empty())));
    }

    /// 回归(红→蓝路径之二):换歌后新封面**还在抓取**时,频谱保持上一张封面,
    /// **不**回退到 hue——这样等新色板就绪能直接红→蓝,而非 hue→蓝。
    #[test]
    fn sync_spectrum_holds_previous_cover_until_new_palette_ready() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let red = MediaUrl::remote("https://example.com/red.jpg")?;
        let blue = MediaUrl::remote("https://example.com/blue.jpg")?;
        // 当前在播这首歌封面是 blue,但频谱上一张应用的是 red。
        if let Some(song) = app.state.current.as_mut() {
            song.cover_url = Some(blue);
        }
        app.state.spectrum_cover = Some(red.clone());
        // blue 的色板 / 图都还没到 —— sync 应原地保持(不清、不抢先标记)。
        app.sync_spectrum_palette();
        assert_eq!(
            app.state.spectrum_cover.as_ref(),
            Some(&red),
            "抓图途中应保持上一张封面"
        );
        Ok(())
    }

    /// 回归(红→蓝路径之一):新封面色板就绪即触发过渡并记下其 key。
    #[test]
    fn sync_spectrum_begins_transition_when_palette_ready() -> color_eyre::Result<()> {
        let mut app = app_with_queue(/*len*/ 1, /*current_idx*/ 0)?;
        let blue = MediaUrl::remote("https://example.com/blue.jpg")?;
        if let Some(song) = app.state.current.as_mut() {
            song.cover_url = Some(blue.clone());
        }
        let palette = CoverPalette::new(vec![Rgb::new(20, 20, 120), Rgb::new(40, 40, 200)])
            .ok_or_else(|| color_eyre::eyre::eyre!("非空色板"))?;
        app.state.cover_palettes.insert(blue.clone(), palette);
        app.sync_spectrum_palette();
        assert_eq!(
            app.state.spectrum_cover.as_ref(),
            Some(&blue),
            "色板就绪应记下并触发过渡"
        );
        Ok(())
    }

    /// 回归:全屏下关闭居中浮层(quit 确认)后,封面协议缓存被清空 —— 据此下一帧重建并全量
    /// re-place,消除「居中浮层压过封面中段、关闭后 kitty 行不自重发」留下的残影。
    #[test]
    fn fullscreen_overlay_close_clears_cover_protocol() -> color_eyre::Result<()> {
        use mineral_model::MediaUrl;

        use crate::test_support::app_in_fullscreen;

        let mut app = app_in_fullscreen()?;
        assert!(app.state.fullscreen, "前置:已稳态进入全屏");

        // 模拟封面已渲染:塞一个协议缓存条目。
        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(32, 32));
        let proto = app.picker.new_resize_protocol(img);
        app.state
            .cover_protocols
            .borrow_mut()
            .insert(url, (proto, (10, 10)));
        assert!(
            !app.state.cover_protocols.borrow().is_empty(),
            "前置:封面协议条目已就位"
        );

        // 开一个居中浮层(quit 确认)并推满进场动画。开着的全程 len 不减,协议缓存不应被动。
        app.overlays.push(super::OverlayKind::confirm());
        for _ in 0..40 {
            app.tick_overlays();
        }
        assert!(
            !app.state.cover_protocols.borrow().is_empty(),
            "浮层开着时(未出栈)不应清空封面协议"
        );

        // 关闭并推满退场动画 → 浮层出栈 → 该拍清空封面协议。
        app.overlays.close_top();
        for _ in 0..40 {
            app.tick_overlays();
        }
        assert!(
            app.state.cover_protocols.borrow().is_empty(),
            "全屏关浮层后封面协议应被清空(触发重 place 消残影)"
        );
        Ok(())
    }

    /// 回归:全屏下关闭**停靠**浮层(queue,贴右不碰封面)**不应**清空封面协议 —— 清了会白白
    /// 触发封面重新解码 / base64 编码,造成关闭动画途中全局卡顿。
    #[test]
    fn fullscreen_queue_close_keeps_cover_protocol() -> color_eyre::Result<()> {
        use mineral_model::MediaUrl;

        use crate::test_support::app_in_fullscreen;

        let mut app = app_in_fullscreen()?;

        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::new(32, 32));
        let proto = app.picker.new_resize_protocol(img);
        app.state
            .cover_protocols
            .borrow_mut()
            .insert(url, (proto, (10, 10)));

        // 开「停靠」队列浮层并推满进场,再关闭并推满退场 → 出栈。
        app.overlays.push(super::OverlayKind::queue(/*sel*/ 0));
        for _ in 0..40 {
            app.tick_overlays();
        }
        app.overlays.close_top();
        for _ in 0..40 {
            app.tick_overlays();
        }

        assert!(
            !app.state.cover_protocols.borrow().is_empty(),
            "停靠浮层(queue)出栈不应清空封面协议(贴右不碰封面,清了徒增重编码卡顿)"
        );
        Ok(())
    }

    /// 集成回归:Tab 开队列 → 按键经 dispatch 路由到 queue 浮层移动光标,且**不被
    /// server sync tick 弹回**。此前 apply 每帧用 server 的
    /// 「在播锚点」覆盖 UI 光标,导致按键看似无效;现在光标归 overlay 私有、只 clamp。
    #[test]
    fn queue_nav_moves_and_survives_snapshot_tick() -> color_eyre::Result<()> {
        // queue 6 首,当前在播第 2 首(idx 2)。
        let mut app = app_with_queue(6, /*current_idx*/ 2)?;

        // Tab 打开浮层:光标定位到在播行。
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.overlays.queue_sel(), Some(2), "打开时光标应落在在播歌");

        // j 两次 → 4。
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.overlays.queue_sel(), Some(4));

        // 模拟一次 server tick:sync 带不同的 queue_sel(在播锚点 = 2)+ queue 重段。
        // UI 光标不应被这个值覆盖(只 clamp 防越界)。
        let sync = PlayerSync {
            queue: Some(QueueSync {
                queue: endserenading(6),
                original_queue: None,
            }),
            queue_sel: 2,
            ..Default::default()
        };
        app.apply_player_sync(sync);
        assert_eq!(
            app.overlays.queue_sel(),
            Some(4),
            "sync tick 不该弹回 UI 光标"
        );

        // k 一次 → 3;g → 0;G → 末行 5。
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.overlays.queue_sel(), Some(3));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.overlays.queue_sel(), Some(0));
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.overlays.queue_sel(), Some(5));

        Ok(())
    }

    /// 版本门控的关键语义回归:重段缺席(版本一致的稳态 tick)= 「与已有一致」,
    /// **不是清空** —— queue / current 镜像必须原地保持。
    #[test]
    fn light_only_sync_keeps_queue_and_current() -> color_eyre::Result<()> {
        let mut app = app_with_queue(6, /*current_idx*/ 2)?;
        let queue_before = app.state.queue.len();
        let current_before = app.state.current.clone();
        assert!(current_before.is_some(), "前置:有在播歌");

        // 稳态 tick:两重段都缺席,只有轻段。
        app.apply_player_sync(PlayerSync::default());

        assert_eq!(app.state.queue.len(), queue_before, "queue 不得被清空");
        assert_eq!(app.state.current, current_before, "current 不得被清空");
        Ok(())
    }

    /// 带重段的 sync 正常替换镜像 + 记录版本号供下次回报。
    #[test]
    fn sync_with_sections_replaces_and_records_versions() -> color_eyre::Result<()> {
        let mut app = app_with_queue(2, /*current_idx*/ 0)?;
        let sync = PlayerSync {
            versions: mineral_protocol::PlayerVersions {
                queue: 7,
                current: 9,
            },
            queue: Some(QueueSync {
                queue: endserenading(4),
                original_queue: None,
            }),
            ..Default::default()
        };
        app.apply_player_sync(sync);
        assert_eq!(app.state.queue.len(), 4, "queue 重段应整体替换");
        assert_eq!(app.state.versions.queue, 7);
        assert_eq!(app.state.versions.current, 9);
        Ok(())
    }

    /// 集成回归:Tab 开 queue → Esc 关闭(触发收起动画);q 在无浮层时开退出确认。
    #[test]
    fn tab_opens_queue_esc_closes() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        // 无浮层时 q 开退出确认 —— 不应直接退出。
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.should_quit, "q 应开退出确认而非直接退出");
        // n 取消(关闭确认)。
        press(&mut app, KeyCode::Char('n'));

        // Tab 开 queue,Esc 关闭后光标进入退场,不再接收导航。
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.overlays.queue_sel(), Some(0));
        press(&mut app, KeyCode::Esc);
        // 收起动画归零前推进若干 tick,最终栈清空、queue 不再存在。
        for _ in 0..16 {
            app.overlays.tick();
        }
        assert_eq!(app.overlays.queue_sel(), None, "Esc 后 queue 应收起并移除");
        Ok(())
    }

    /// 退出收缩:q → confirm → y 不立即退,而是启动收缩动画;推进到归零后才退出。
    #[test]
    fn quit_plays_shrink_animation_then_exits() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        press(&mut app, KeyCode::Char('q'));
        press(&mut app, KeyCode::Char('y'));
        assert!(!app.should_quit, "确认退出应先播收缩动画,不立即退");
        assert!(
            matches!(&app.transition, Some(t) if t.leaving()),
            "应进入退出(收缩)转场态"
        );

        // 模拟主循环逐 tick 推进转场,归零后置退出并清空转场。
        for _ in 0..40 {
            if app.transition.is_some() {
                app.tick_transition();
            }
        }
        assert!(app.should_quit, "收缩动画归零后应退出");
        assert!(app.transition.is_none(), "收尾后转场应清空");
        Ok(())
    }

    /// 启动扩大:进入扩大转场(非退场),推进到满后清空转场、不退出、转入正常运行。
    #[test]
    fn startup_expand_plays_then_runs_normally() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        app.transition = Some(Transition::expanding(TRANSITION_TICKS));
        assert!(
            matches!(&app.transition, Some(t) if !t.leaving()),
            "启动应是扩大(进场)转场"
        );

        for _ in 0..40 {
            if app.transition.is_some() {
                app.tick_transition();
            }
        }
        assert!(app.transition.is_none(), "扩大动画结束应清空转场");
        assert!(!app.should_quit, "启动动画结束不应退出");
        Ok(())
    }

    /// Ctrl-C 立即退出,不走转场动画。
    #[test]
    fn ctrl_c_exits_immediately_without_animation() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(app.should_quit, "Ctrl-C 立即退出");
        assert!(app.transition.is_none(), "Ctrl-C 不走转场动画");
        Ok(())
    }

    /// vim 命令行行为:`/` 进搜索,删到空仍在搜索态,空 query 上再删一次才退出。
    #[test]
    fn search_backspace_on_empty_query_exits() -> color_eyre::Result<()> {
        let mut app = app_with_library(3, /*sel_track*/ 0)?;

        // `/` 进入搜索输入态,query 起始为空。
        press(&mut app, KeyCode::Char('/'));
        assert!(app.state.search_mode, "`/` 应进入搜索态");
        assert!(app.state.search_q.is_empty());

        // 输入两个字符。
        press(&mut app, KeyCode::Char('a'));
        press(&mut app, KeyCode::Char('b'));
        assert_eq!(app.state.search_q, "ab");

        // 退格逐字符删;删到空时仍停在搜索态(不提前退出)。
        press(&mut app, KeyCode::Backspace);
        press(&mut app, KeyCode::Backspace);
        assert!(app.state.search_q.is_empty());
        assert!(app.state.search_mode, "删到空时仍应在搜索态");

        // 空 query 上再删一次 → 退出搜索。
        press(&mut app, KeyCode::Backspace);
        assert!(!app.state.search_mode, "空 query 上退格应退出搜索");
        Ok(())
    }

    /// Library 视图按 `f` 乐观切换选中曲目的 ♥ 状态,不依赖真实 server。
    /// 第一次按:`loved` 从 false → true;再按一次:true → false。
    #[test]
    fn pressing_f_toggles_loved_optimistically() -> color_eyre::Result<()> {
        // 3 首曲目,选中第 0 首(初始 loved=false,TestClient::toggle_love 是 no-op)。
        let mut app = app_with_library(3, /*sel_track*/ 0)?;

        // 取第 0 首曲目 id,用于后续断言 liked_ids。
        let song_id = app
            .state
            .filtered_tracks()
            .first()
            .map(|sv| sv.data.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 没有曲目"))?;

        // 初始 loved = false。
        assert!(
            !app.state
                .liked_ids
                .get(&SourceKind::NETEASE)
                .is_some_and(|s| s.contains(&song_id)),
            "初始不应在 liked_ids 里"
        );

        // 按 f → 乐观翻转成 loved。
        press(&mut app, KeyCode::Char('f'));
        assert!(
            app.state
                .liked_ids
                .get(&SourceKind::NETEASE)
                .is_some_and(|s| s.contains(&song_id)),
            "按 f 后应进入 liked_ids"
        );
        let loved_after_first = app
            .state
            .filtered_tracks()
            .first()
            .is_some_and(|sv| sv.loved);
        assert!(loved_after_first, "第一次按 f 后 SongView.loved 应为 true");

        // 再按 f → 翻转回 not loved。
        press(&mut app, KeyCode::Char('f'));
        assert!(
            !app.state
                .liked_ids
                .get(&SourceKind::NETEASE)
                .is_some_and(|s| s.contains(&song_id)),
            "再按 f 后应从 liked_ids 中移除"
        );
        let loved_after_second = app
            .state
            .filtered_tracks()
            .first()
            .is_some_and(|sv| sv.loved);
        assert!(
            !loved_after_second,
            "第二次按 f 后 SongView.loved 应为 false"
        );

        Ok(())
    }

    /// 列表导航逐键回归:j/k/J/K/g/G 在 Library 与 Playlists 两个视图移动选中,
    /// 大跨步长 7、越界钳到首末行。
    #[test]
    fn j_k_navigate_in_playlists_and_library() -> color_eyre::Result<()> {
        // Library:10 首,从 0 起。
        let mut app = app_with_library(10, /*sel_track*/ 0)?;
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.sel_track, 1, "j 下移一行");
        press(&mut app, KeyCode::Char('J'));
        assert_eq!(app.state.sel_track, 8, "J 大跨下移 7 行");
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.sel_track, 9, "下移越界钳到末行");
        press(&mut app, KeyCode::Char('K'));
        assert_eq!(app.state.sel_track, 2, "K 大跨上移 7 行");
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.state.sel_track, 1, "k 上移一行");
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.state.sel_track, 0, "g 跳首行");
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.state.sel_track, 9, "G 跳末行");

        // Playlists:3 张歌单,同一组键作用于 sel_playlist。
        let mut app = app_with_library(3, /*sel_track*/ 0)?;
        app.state.view = crate::runtime::state::View::Playlists;
        app.state.playlists = vec![
            crate::test_support::playlist_view("p1", "A", SourceKind::NETEASE, 1),
            crate::test_support::playlist_view("p2", "B", SourceKind::NETEASE, 1),
            crate::test_support::playlist_view("p3", "C", SourceKind::NETEASE, 1),
        ];
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.state.sel_playlist, 1, "j 下移一行");
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(app.state.sel_playlist, 2, "G 跳末行");
        press(&mut app, KeyCode::Char('k'));
        assert_eq!(app.state.sel_playlist, 1, "k 上移一行");
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.state.sel_playlist, 0, "g 跳首行");
        Ok(())
    }

    /// 音量 / seek 逐键回归:`+`/`=`/`-`/`_` 走本地乐观值(±5 钳 0..=100);
    /// `←`/`→`/`Shift+←`/`Shift+→` 只发 server 命令,本地 position 无回显。
    #[test]
    fn volume_and_seek_via_keymap() -> color_eyre::Result<()> {
        let mut app = app_with_queue(1, /*current_idx*/ 0)?;
        app.state.playback.volume_pct = 50;
        press(&mut app, KeyCode::Char('+'));
        assert_eq!(app.state.playback.volume_pct, 55, "+ 加 5");
        press(&mut app, KeyCode::Char('='));
        assert_eq!(app.state.playback.volume_pct, 60, "= 与 + 同义");
        press(&mut app, KeyCode::Char('-'));
        assert_eq!(app.state.playback.volume_pct, 55, "- 减 5");
        press(&mut app, KeyCode::Char('_'));
        assert_eq!(app.state.playback.volume_pct, 50, "_ 与 - 同义");

        // seek 是 server 往返,本地 position 不乐观回显;此处只确认按键被消化不 panic。
        app.state.playback.position_ms = 60_000;
        press(&mut app, KeyCode::Left);
        press(&mut app, KeyCode::Right);
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Left,
            KeyModifiers::SHIFT,
        )));
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Right,
            KeyModifiers::SHIFT,
        )));
        assert_eq!(
            app.state.playback.position_ms, 60_000,
            "seek 无本地回显(等 snapshot)"
        );
        Ok(())
    }

    /// Playlists 视图 `l` 进 Library;Library 视图 Enter 触发 set_queue+play
    /// (TestClient no-op,断 view 流转与不 panic)。
    #[test]
    fn l_enters_library_enter_plays() -> color_eyre::Result<()> {
        let mut app = app_with_library(3, /*sel_track*/ 2)?;
        app.state.view = crate::runtime::state::View::Playlists;
        app.state.sel_playlist = 0;

        press(&mut app, KeyCode::Char('l'));
        assert_eq!(
            app.state.view,
            crate::runtime::state::View::Library,
            "l 应进 Library"
        );
        assert_eq!(app.state.sel_track, 0, "进 Library 选中复位到首行");

        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.state.view,
            crate::runtime::state::View::Library,
            "Enter 播放不切视图"
        );
        Ok(())
    }

    /// `d` 按视图分流下载意图(Playlists→歌单 / Library→单曲),TestClient no-op:
    /// 断不 panic、选中与视图不变(不验 Client 调用细节,见 sub00 §10 spy 裁决)。
    #[test]
    fn d_downloads_selection_by_view() -> color_eyre::Result<()> {
        let mut app = app_with_library(3, /*sel_track*/ 1)?;
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.state.sel_track, 1, "Library d 不动选中");
        assert_eq!(
            app.state.view,
            crate::runtime::state::View::Library,
            "Library d 不切视图"
        );

        let mut app = app_with_library(3, /*sel_track*/ 0)?;
        app.state.view = crate::runtime::state::View::Playlists;
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.state.sel_playlist, 0, "Playlists d 不动选中");
        assert_eq!(
            app.state.view,
            crate::runtime::state::View::Playlists,
            "Playlists d 不切视图"
        );
        Ok(())
    }

    /// `z` 进/退全屏(toggle):进场目标置满(非 leaving),再按退场目标归零(leaving)。
    #[test]
    fn z_toggles_fullscreen() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        assert!(!app.state.fullscreen, "初始非全屏");

        press(&mut app, KeyCode::Char('z'));
        assert!(app.state.fullscreen, "z 进全屏");
        assert!(!app.state.fullscreen_pos.leaving(), "进场:形变目标置满");

        press(&mut app, KeyCode::Char('z'));
        assert!(!app.state.fullscreen, "再按 z 退全屏");
        assert!(app.state.fullscreen_pos.leaving(), "退场:形变目标归零");
        Ok(())
    }

    /// 全屏态屏蔽列表导航 + 搜索 `/`;
    #[test]
    fn fullscreen_blocks_nav_and_search() -> color_eyre::Result<()> {
        let mut app = app_with_library(5, /*sel_track*/ 2)?;
        press(&mut app, KeyCode::Char('z'));
        assert!(app.state.fullscreen);

        // j / g 导航被吞,选中不变。
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('g'));
        assert_eq!(app.state.sel_track, 2, "全屏屏蔽列表导航");

        // `/` 不进搜索态。
        press(&mut app, KeyCode::Char('/'));
        assert!(!app.state.search_mode, "全屏屏蔽搜索 `/`");
        Ok(())
    }

    /// 全屏态内 `Tab` 仍打开 queue 浮层(浮层是独立层),光标落在在播歌。
    #[test]
    fn fullscreen_tab_still_opens_queue() -> color_eyre::Result<()> {
        let mut app = app_with_queue(4, /*current_idx*/ 1)?;
        press(&mut app, KeyCode::Char('z'));
        assert!(app.state.fullscreen);

        press(&mut app, KeyCode::Tab);
        assert_eq!(
            app.overlays.queue_sel(),
            Some(1),
            "全屏内 Tab 仍开 queue,光标在在播歌"
        );
        Ok(())
    }
}

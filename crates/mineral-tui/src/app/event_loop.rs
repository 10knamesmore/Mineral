//! 同步主事件循环：绘制、后端同步、动画推进与终端状态上报。
//!
//! 播放、队列、自动续播与预取由 daemon 负责；每帧读取本地订阅镜像，绘制与输入无需等待 IPC 应答。

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEventKind};

use crate::components::popup::OverlayKind;
use crate::render::anim::{Transition, ticks16_from_ms};
use crate::runtime::window_title::TitleContext;
use crate::tui::Tui;
use crate::view::draw;

use super::App;

impl App {
    /// 主循环帧间隔(现读配置 `animation.frame_tick_ms`,热更下一帧生效)。
    fn frame_tick(&self) -> Duration {
        Duration::from_millis(*self.state.cfg.tui().animation().frame_tick_ms())
    }

    /// 整屏转场拍数(现读配置 `animation.transition_ms` 按帧率折算;
    /// 只在启停时构造转场,现场折算即可)。
    pub(super) fn transition_ticks(&self) -> u16 {
        let anim = self.state.cfg.tui().animation();
        ticks16_from_ms(*anim.transition_ms(), *anim.frame_tick_ms())
    }

    /// 同步主事件循环:绘制 → 等事件 → 每帧间隔拉数据 + 推进动画/频谱
    /// (节奏由配置 `animation.frame_tick_ms` 决定,默认 ~60fps)。
    pub fn run(&mut self, tui: &mut Tui) -> color_eyre::Result<()> {
        // 启动时先灌一次镜像(订阅已在连接后建立,首帧通常已到达)。
        self.sync_from_backend();

        // 启动扩大转场:界面从中心小框向四周铺满,与退出收缩反向对称。推满后转入正常运行。
        self.transition = Some(Transition::expanding(self.transition_ticks()));

        // client 侧心跳(间隔 = daemon.heartbeat_secs):报 server 看不到的 UI / 缓存状态(启动即首条)。
        let mut last_heartbeat = Instant::now();
        self.log_heartbeat();

        // 启动上报一次终端状态(此后 Resize / 全屏切换增量上报)。
        self.report_terminal_state();

        // SIGTERM / SIGINT / SIGHUP 由 watcher 记日志并置标志,主循环据此走正常退出,
        // 让 `Tui::exit` 有机会还原终端。
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
            // 窗口标题与 ratatui draw 走不同通道,在 draw 之前单独更新。
            // 歌词行仅在有模板引用它时才按需拼接(拼接要定位当前行 + 拥有化文本),
            // 先落局部持有,再借进 context。
            let title_lyric = if self.window_title.wants_lyric() {
                self.state.active_title_lyric()
            } else {
                None
            };
            let title_ctx = TitleContext {
                song: self.state.player.current.as_ref(),
                playing: self.state.playback.playing,
                connected: self.client.connected(),
                position_ms: self.state.playback.position_ms,
                duration_ms: self.state.playback.duration_ms(),
                lyric: title_lyric.as_deref(),
                override_text: self.state.window_title_override.as_deref(),
            };
            // 标题启用时确保标题栈已 push（幂等）——启动即启用由 lib.rs 兜住，热重载
            // 从禁用→启用则由此补上，保证退出能对称 pop 还原原标题。禁用时不 push。
            if self.window_title.enabled() {
                tui.push_title_stack()?;
            }
            self.window_title.update(&title_ctx)?;
            if self.overlays.is_disconnected() {
                // 只渲染断连提示 + 推进其弹出动画 + 等按键退出;daemon 没了,正常路径全是
                // 兜底默认值,跳过后端同步。fatal 态直接退出(不走 dispatch,不玩退出收缩动画)。
                // 清掉转场:本分支不推进它,启动即断连否则会把扩大动画卡在空屏。
                self.transition = None;
                tui.draw(|f| {
                    draw(f, self);
                    self.state.images.flush_graphics_commands()
                })?;
                if event::poll(self.frame_tick())?
                    && let Event::Key(key) = event::read()?
                    && key.kind == KeyEventKind::Press
                {
                    self.should_quit = true;
                }
                self.overlays.tick();
                continue;
            }

            tui.draw(|f| {
                draw(f, self);
                self.state.images.flush_graphics_commands()
            })?;

            let timeout = self.frame_tick().saturating_sub(self.last_tick.elapsed());
            if event::poll(timeout)? {
                self.handle_event(&event::read()?);
            }
            if self.last_tick.elapsed() >= self.frame_tick() {
                // 转场动画(启动扩大 / 退出收缩)进行中:只推进它 + 重绘(上方 tui.draw),
                // 跳过后端同步;退出转场归零即退,启动转场推满即转入正常运行。
                if self.transition.is_some() {
                    self.tick_transition();
                    self.last_tick = Instant::now();
                    continue;
                }
                self.drain_push_events();
                self.drain_completions();
                self.sync_from_backend();
                self.update_spectrum();
                self.state.tick_frame();
                self.tick_overlays();
                self.tick_images();
                self.notifications.tick();
                // 每 tick 抄一份本地钟点,供队列剩余时长算「预计播完钟点」(渲染只持 &state)。
                self.state.now.set(chrono::Local::now());
                self.last_tick = Instant::now();
                // 心跳间隔现读配置(daemon.heartbeat_secs),热更下一轮生效。
                let heartbeat = Duration::from_secs(*self.state.cfg.daemon().heartbeat_secs());
                if last_heartbeat.elapsed() >= heartbeat {
                    self.log_heartbeat();
                    last_heartbeat = Instant::now();
                }
            }
        }
        Ok(())
    }

    /// 推进图片引擎、图片相关预取与封面配色消费。
    fn tick_images(&mut self) {
        let current_cover = self
            .state
            .playback
            .track
            .as_ref()
            .and_then(|track| track.cover_url.clone());
        let fullscreen_stable = self.state.browse.fullscreen.at_max();
        self.state.images.tick(current_cover, fullscreen_stable);
        let queue_covers = self.overlays.queue_cover_candidates(&self.state);
        crate::runtime::prefetch::tick(&mut self.state, &*self.client, queue_covers);
        self.sync_cover_palette();
        self.tick_cover_fades();
    }

    /// 推进浮层动画一拍;并处理「全屏下居中浮层刚被移除」的封面残影。
    ///
    /// 居中浮层(如 quit 确认)在全屏时会压住左侧封面的中段。kitty 协议把整行 unicode
    /// 占位符打包在该行**最左 cell**、其余 cell `set_skip(true)`,而 ratatui 的 buffer diff
    /// 跳过未变 cell —— 浮层只盖了封面中段、没碰最左驱动 cell,关闭后那几行不会自行重发,
    /// 中段残留浮层底色(残影)。故在该居中浮层(退场动画放完)真正出栈的那一拍,清一次封面
    /// 终端图片缓存：下一帧由 [`crate::image::ImageEngine`] 按需重建、重新 transmit 并
    /// 全量 re-place，消除残影。
    ///
    /// **仅对居中浮层做此事**:停靠浮层(queue 贴右)不压封面,清它纯属白白触发封面重新解码
    /// / 终端协议编码(几十毫秒、卡掉一帧),故停靠浮层出栈不刷新。
    fn tick_overlays(&mut self) {
        let before = self.overlays.len();
        let closing_centered = self.overlays.any_leaving_centered();
        self.overlays.tick();
        if self.state.browse.fullscreen.on() && closing_centered && self.overlays.len() < before {
            self.state.images.terminal_images.clear();
        }
    }

    /// 推进整屏转场动画一帧。`settled()`(进度抵达目标)时收尾:退出转场(`leaving()`)置
    /// `should_quit`,启动转场转入正常运行;两者随后统一清空 `transition`。无转场时为空操作。
    pub(super) fn tick_transition(&mut self) {
        let Some(anim) = &mut self.transition else {
            return;
        };
        anim.tick();
        if anim.settled() {
            if anim.leaving() {
                // Shift+Q 的「退出并停止 daemon」在动画收尾时才真正投递——
                // 与退出同一时点;Ctrl-C / 断连不经此路径,不会误杀。
                if self.stop_daemon_on_quit {
                    self.client.request_daemon_shutdown();
                }
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
            .library
            .liked_ids
            .values()
            .fold(0_usize, |acc, set| acc + set.len());
        mineral_log::info!(
            target: "heartbeat",
            view = ?s.browse.view,
            playlists = s.library.playlists.len(),
            tracks_cached = s.library.tracks.len(),
            tracks_requested = s.library.tracks_requested.len(),
            lyrics_cached = s.library.lyrics.len(),
            covers_cached = s.images.cache.len(),
            covers_pending = s.images.loading_count(),
            liked,
            queue_len = s.player.queue.len(),
            events_dropped = self.client.events_dropped(),
            "client status"
        );
    }

    /// 上报终端 UI 状态(尺寸 + 全屏态 + 焦点)给 daemon,灌属性树 `terminal` 供脚本
    /// observe。值没变去抖不发;无 TTY(测试)拿不到尺寸静默跳过。
    /// 调用点:启动 / Resize / 全屏切换 / focus 变化。
    pub(super) fn report_terminal_state(&mut self) {
        let Ok((cols, rows)) = crossterm::terminal::size() else {
            return;
        };
        let snapshot = (
            rows,
            cols,
            self.state.browse.fullscreen.on(),
            self.state.focused(),
        );
        if self.last_terminal_report == Some(snapshot) {
            return;
        }
        self.last_terminal_report = Some(snapshot);
        self.client.report_terminal_state(
            rows,
            cols,
            self.state.browse.fullscreen.on(),
            self.state.focused(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::Transition;
    use crate::test_support::app_with_queue;

    /// 测试对照值 = default.lua 的 `animation.transition_ms`(288)÷ `frame_tick_ms`(16)。
    const TRANSITION_TICKS: u16 = 18;

    /// 回归：全屏下关闭居中浮层(quit 确认)后，终端图片缓存被清空，据此下一帧重建并全量
    /// re-place,消除「居中浮层压过封面中段、关闭后 kitty 行不自重发」留下的残影。
    #[test]
    fn fullscreen_overlay_close_clears_cover_protocol() -> color_eyre::Result<()> {
        use mineral_model::MediaUrl;

        use crate::test_support::app_in_fullscreen;

        let mut app = app_in_fullscreen()?;
        assert!(app.state.browse.fullscreen.on(), "前置:已稳态进入全屏");

        // 模拟封面已渲染：塞一个终端图片缓存条目。
        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        app.state.images.insert_test_terminal_image(&url, (10, 10));
        assert!(
            !app.state.images.terminal_images.is_empty(),
            "前置:封面协议条目已就位"
        );

        // 开一个居中浮层并推满进场动画；浮层尚未出栈时终端图片缓存不应被清空。
        app.overlays.push(super::OverlayKind::confirm());
        for _ in 0..40 {
            app.tick_overlays();
        }
        assert!(
            !app.state.images.terminal_images.is_empty(),
            "浮层开着时(未出栈)不应清空封面协议"
        );

        // 关闭并推满退场动画 → 浮层出栈 → 该拍清空封面协议。
        app.overlays.close_top();
        for _ in 0..40 {
            app.tick_overlays();
        }
        assert!(
            app.state.images.terminal_images.is_empty(),
            "全屏关浮层后封面协议应被清空(触发重 place 消残影)"
        );
        Ok(())
    }

    /// 回归:全屏下关闭**停靠**浮层(queue,贴右不碰封面)**不应**清空封面协议 —— 清了会白白
    /// 触发封面重新解码 / 终端协议编码,造成关闭动画途中全局卡顿。
    #[test]
    fn fullscreen_queue_close_keeps_cover_protocol() -> color_eyre::Result<()> {
        use mineral_model::MediaUrl;

        use crate::test_support::app_in_fullscreen;

        let mut app = app_in_fullscreen()?;

        let url = MediaUrl::remote("https://x.y/c.jpg")?;
        app.state.images.insert_test_terminal_image(&url, (10, 10));

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
            !app.state.images.terminal_images.is_empty(),
            "停靠浮层(queue)出栈不应清空封面协议(贴右不碰封面,清了徒增重编码卡顿)"
        );
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
}

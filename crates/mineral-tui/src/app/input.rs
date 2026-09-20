//! 终端事件与按键分发：文本输入、页面动作、浮层意图和退出操作。

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::components::popup::{OverlayAction, OverlayKind, OverlayResponse};
use crate::render::anim::Transition;
use crate::runtime::action::{Action, SeekDelta, VolumeDelta};
use crate::runtime::keymap::{Keymap, chord_from_event};
use crate::runtime::state::PageKind;

use super::{App, menus};

impl App {
    /// 处理一个 crossterm 事件:KeyEvent 的按下边沿走按键分发;Resize / focus
    /// 变化上报 daemon(脚本经 `terminal` 属性观察终端尺寸与焦点)。
    pub(super) fn handle_event(&mut self, ev: &Event) {
        match ev {
            Event::Key(key) if key.kind == KeyEventKind::Press => self.handle_key(key),
            Event::Resize(..) => {
                self.state.images.refresh_cell_pixels();
                self.report_terminal_state();
            }
            Event::FocusGained => self.set_focus(/*focused*/ true),
            Event::FocusLost => self.set_focus(/*focused*/ false),
            _ => {}
        }
    }

    /// focus 事件落地:起顶栏变灰淡入/淡出(`dim` 开 = 未聚焦)、上报 daemon。
    fn set_focus(&mut self, focused: bool) {
        self.state.dim.set(!focused);
        self.report_terminal_state();
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

        // Shift+Q 硬编码逃生口:退出 + 停掉 daemon。不进 keymap(不可重映射、压过
        // 用户绑定)、压过浮层(确认 / queue 开着也直接退);唯独让位文本输入——
        // 文本输入态的大写 Q 是字符,不是退出意图(含 channel-search 搜索框)。只看
        // `Char('Q')` 不看 modifier:部分终端报大写字符时不附带 SHIFT。
        if !self.state.in_text_input() && key.code == KeyCode::Char('Q') {
            self.stop_daemon_on_quit = true;
            // 还停在 Library 内就退出:位置没经过「返回」记录,这里补记。放在转场
            // 起点而非收尾——fire-and-forget 落盘借收缩动画的时长完成,收尾才写
            // 会跟进程退出赛跑。Ctrl-C 强退不经此路径,不保证。
            self.remember_track_pos();
            self.transition = Some(Transition::collapsing(self.transition_ticks()));
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

        // —— 以下:无活跃浮层 —— 按当前页(page_kind)路由到对应 Page 实现。Browse 页内部再按
        // 子模式(deep-search 打字 / 其余)细分;子模式上下文裁决(全屏屏蔽列表导航等)仍在各执行器判。
        match self.state.page_kind() {
            // 搜索框(prompt 焦点)是模态文本输入吞键进 query;results/detail 焦点只截获
            // 面板导航、其余回落全局,见 channel_search。
            PageKind::Search => self.handle_channel_search_key(key),
            // Browse 页(含 fullscreen / `/` 过滤子模式):on_key 内部按子模式分流。
            PageKind::Browse => self.handle_browse_key(key),
        }
    }

    /// 执行一个查表命中的 [`Action`]:每分支一行调用对应执行器,逻辑都在执行器里。
    pub(super) fn dispatch(&mut self, action: Action) {
        match action {
            Action::ToggleFullscreen => self.toggle_fullscreen(),
            Action::OpenSearchView => self.open_search_view(),
            Action::OpenQueue => self.open_queue(),
            Action::OpenDownloads => self.open_downloads(),
            Action::OpenQuitConfirm => self.overlays.push(OverlayKind::confirm()),
            Action::CycleLyricExtra => self.cycle_lyric_extra(),
            Action::Scroll(step) => self.scroll(step),
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
            Action::DismissNotice => self.dismiss_notice(),
            Action::OpenActionMenu => self.open_menu(menus::MenuKind::Action),
            Action::OpenCopyMenu => self.open_menu(menus::MenuKind::Copy),
            Action::InvokeScript(slot) => self.invoke_script_action(slot),
            Action::OpenHelp => self.open_help(),
            // 仅 search 面板内有意义(由 handle_search_panel_key 拦截消费);其它布局态落此 = no-op。
            Action::DrillIntoSelection | Action::CycleDetailSection => {}
            // 仅 queue 浮层内有意义(由其 on_action 消费);其它布局态落此 = no-op。
            Action::ReorderSelection(_) => {}
            // `c` 跳在播曲:queue 浮层自管,Browse 页落到曲目列表(歌单面没有在播概念)。
            Action::JumpToCurrent if matches!(self.state.page_kind(), PageKind::Browse) => {
                self.jump_to_current();
            }
            Action::JumpToCurrent => {}
        }
    }

    /// 关最早一张驻留通知卡片(连按逐条关;无卡空操作)。
    fn dismiss_notice(&mut self) {
        let _ = self.notifications.dismiss_card();
    }

    /// 由 keymap 反查 [`Action::DismissNotice`] 绑定键,合成卡片底边提示
    /// (裸键名,如 `x`);用户解绑后为空串(卡片不画 footer)。
    pub(crate) fn compose_notice_hint(keymap: &Keymap) -> String {
        keymap
            .hint_chord(Action::DismissNotice)
            .map(|c| c.to_string())
            .unwrap_or_default()
    }

    /// 切换全屏播放态:翻转开关并驱动形变进退场(`eased_in_out`,可中途反向)。
    /// search 布局态下屏蔽(与 search 互斥,见 [`Self::open_search_view`])。
    fn toggle_fullscreen(&mut self) {
        if self.state.channel_search.active.on() {
            return;
        }
        self.state.browse.fullscreen.toggle();
        mineral_log::debug!(
            target: "tui",
            fullscreen = self.state.browse.fullscreen.on(),
            progress = self.state.browse.fullscreen.raw(),
            idle_vinyl = self.state.playback.track.is_none(),
            "fullscreen layout transition"
        );
        self.report_terminal_state();
    }

    /// 切换 Search 布局态:翻转开关驱动布局端点 morph 进退场。全屏态下屏蔽
    /// (两个全屏级布局态互斥,逻辑 `on` 同时只一个)。
    fn open_search_view(&mut self) {
        if self.state.browse.fullscreen.on() {
            return;
        }
        let entering_from_browse = self.state.channel_search.active.at_min();
        self.state.channel_search.active.toggle();
        mineral_log::debug!(
            target: "tui",
            search = self.state.channel_search.active.on(),
            progress = self.state.channel_search.active.raw(),
            "search layout transition"
        );
        // 从浏览端点进入时建立会话；中途反向沿用搜索焦点与输入状态。
        if self.state.channel_search.active.on() && entering_from_browse {
            self.state.channel_search.enter(&self.state.caps);
        }
    }

    /// `t` 键:循环歌词副轨档,并把新档落盘(跨会话保留,fire-and-forget)。
    fn cycle_lyric_extra(&mut self) {
        self.state.cycle_lyric_extra();
        self.ui_prefs
            .save_lyric_extra(self.state.browse.lyric_view.extra);
    }

    /// 执行浮层产生的意图(浮层自身不持有 App,按键产出意图回这里执行)。
    fn run_overlay_action(&mut self, action: OverlayAction) {
        match action {
            // 正常退出:不立即退,而是启动「边框向中心收缩」退场动画,归零后主循环再 break。
            // 补记 Library 内的光标位置,时点理由见 Shift+Q 路径。
            OverlayAction::Quit => {
                self.remember_track_pos();
                self.transition = Some(Transition::collapsing(self.transition_ticks()));
            }
            OverlayAction::CloseTop => self.overlays.close_top(),
            OverlayAction::StopDownload(id) => {
                self.client.stop_download(id);
            }
            OverlayAction::PlayQueueIndex(i) => {
                if let Some(song) = self.state.player.queue.get(i).cloned() {
                    self.client.play_song(song);
                }
            }
            // queue 浮层 `y`:复制菜单叠在 queue 之上(不关 queue),为该队列项构造复制项。
            OverlayAction::CopyQueueIndex { idx, anchor } => {
                self.open_queue_copy_menu(idx, anchor);
            }
            // 队列编辑族(含叠在 queue 之上的操作菜单):整族交给 queue_edit 模块。
            edit @ (OverlayAction::QueueActionMenu { .. }
            | OverlayAction::ToggleLoveQueueIndex(_)
            | OverlayAction::DownloadQueueIndex(_)
            | OverlayAction::ReorderQueueIndex { .. }) => self.run_queue_overlay_action(&edit),
            // 菜单确认即收:先关菜单(收起动画),再执行选中动作。
            OverlayAction::Menu(action) => {
                self.overlays.close_top();
                self.run_menu_action(action);
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

    /// 半穿透白名单:歌词切换 + 播放控制族 + 通知卡关闭 + cheatsheet(任何半穿透
    /// 浮层上都能叠开);列表 / 视图 / 其余浮层动作不穿透。
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
                | Action::DismissNotice
                | Action::OpenHelp
                | Action::OpenDownloads
        )
    }

    /// 打开浮动播放队列,光标落在上次离开的位置;首次打开(或队列已换)定位到在播歌。
    ///
    /// 记住位置是因为队列翻找往往是连续动作:翻到一半关掉再开,每次被拽回在播行等于
    /// 从头再来。显式回到在播行有 `JumpToCurrent` 专管。
    fn open_queue(&mut self) {
        let fallback = self.state.queue_current_index().unwrap_or(0);
        let sel = self
            .queue_cursor_memo
            .filter(|at| *at < self.state.player.queue.len())
            .unwrap_or(fallback);
        self.overlays.push(OverlayKind::queue(sel));
    }

    /// Opens the docked flat Song downloads overlay.
    fn open_downloads(&mut self) {
        self.overlays.push(OverlayKind::downloads());
    }

    /// 打开键位 cheatsheet:目录与关闭提示在打开瞬间从 keymap 快照(重映射 /
    /// 脚本绑定自动跟随)。已开在顶时按同键由浮层自身收敛为关闭(toggle),
    /// 不经此路径,无双开之虞。
    fn open_help(&mut self) {
        let close_hint = self
            .keymap
            .hint_chord(Action::OpenHelp)
            .map(crate::components::popup::chip_text);
        self.overlays
            .push(OverlayKind::help(self.keymap.help().to_vec(), close_hint));
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
    use mineral_protocol::{PlayerSync, QueueSync};

    use mineral_model::{SearchKind, SourceKind};

    use super::App;
    use crate::test_support::{
        app_with_library, app_with_queue, app_with_queue_probed, app_with_queue_volume_probed,
        endserenading,
    };

    /// 喂一个 Press 键给 App(走真实事件入口 `handle_event`)。
    fn press(app: &mut App, code: KeyCode) {
        app.handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::empty())));
    }

    /// 喂一个带 Ctrl 的 Press 键给 App(走真实事件入口)。
    fn press_ctrl(app: &mut App, code: KeyCode) {
        app.handle_event(&Event::Key(KeyEvent::new(code, KeyModifiers::CONTROL)));
    }

    /// 集成:queue 光标记忆——翻到别处关掉再开,落回原处而非被拽回在播行。
    #[test]
    fn queue_reopens_at_remembered_cursor() -> color_eyre::Result<()> {
        let mut app = app_with_queue(6, /*current_idx*/ 0)?;
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.overlays.queue_sel(), Some(0), "首次开落在在播行");

        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(app.overlays.queue_sel(), Some(2));
        // 记账挂在每 tick 的 sync 上,先走一拍再关。
        app.apply_player_sync(PlayerSync::default());
        press(&mut app, KeyCode::Tab);
        app.apply_player_sync(PlayerSync::default());
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.overlays.queue_sel(), Some(2), "重开落回上次位置");
        Ok(())
    }

    /// 集成:queue 上按移动键 → 送出带身份定位的 Move,光标跟着歌走。
    #[test]
    fn queue_reorder_key_sends_move_with_anchor() -> color_eyre::Result<()> {
        use crossterm::event::{KeyEvent, KeyModifiers};
        let (mut app, edits) =
            crate::test_support::app_with_queue_edits(6, /*current_idx*/ 0)?;
        press(&mut app, KeyCode::Tab);
        press(&mut app, KeyCode::Char('j'));
        app.handle_key(&KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));

        let sent = edits
            .lock()
            .map_err(|_poisoned| color_eyre::eyre::eyre!("编辑记录被毒化"))?;
        let Some(mineral_protocol::QueueOp::Move { at, to }) = sent.first() else {
            color_eyre::eyre::bail!("应送出一次 Move");
        };
        assert_eq!(at.index, 1, "定位是移动前的下标");
        assert_eq!(to, &mineral_protocol::QueuePos::Down);
        assert_eq!(app.overlays.queue_sel(), Some(2), "光标跟着歌下移");
        Ok(())
    }

    /// queue 浮层行级:操作菜单与复制菜单都叠在 queue 之上(queue 仍在栈底),
    /// 两者都是 resolver 之外的独立接缝——浮层持私有光标,锚点只能由它自算后带回。
    #[test]
    fn queue_action_and_copy_menus_stack_above() -> color_eyre::Result<()> {
        let mut app = app_with_queue(4, /*current_idx*/ 1)?;
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.overlays.len(), 1, "Tab 开 queue 浮层");

        press(&mut app, KeyCode::Char('o'));
        assert_eq!(
            app.overlays.len(),
            2,
            "o 在 queue 之上叠操作菜单(queue 仍在栈底)"
        );

        press(&mut app, KeyCode::Char('y'));
        assert_eq!(
            app.overlays.len(),
            2,
            "y 在 queue 之上叠复制菜单(queue 仍在栈底)"
        );
        Ok(())
    }

    /// queue 光标归 overlay 所有；server sync 更新队列时只钳制越界位置，不能用在播锚点
    /// 覆盖用户刚完成的导航。
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

        // 模拟一次 server tick:sync 带不同的在播锚点(= 2)+ queue 重段。
        // UI 光标不应被这个值覆盖(只 clamp 防越界)。
        let sync = PlayerSync {
            queue: Some(QueueSync {
                queue: endserenading(6),
                original_queue: None,
            }),
            cursor: mineral_protocol::PlayCursor::InQueue(2),
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

    /// 集成:`?` 弹出 cheatsheet → 再按 `?` 收起(toggle);开着时播放控制键半穿透。
    #[test]
    fn question_mark_toggles_help_overlay() -> color_eyre::Result<()> {
        let (mut app, volumes) = app_with_queue_volume_probed(3, /*current_idx*/ 0)?;
        press(&mut app, KeyCode::Char('?'));
        assert!(app.overlays.has_help(), "? 应弹出 cheatsheet");
        // 半穿透:help 开着按音量键仍生效(白名单放行,命令已发出)。
        press(&mut app, KeyCode::Char('-'));
        assert!(
            volumes.lock().is_ok_and(|sent| !sent.is_empty()),
            "音量键应穿透"
        );
        press(&mut app, KeyCode::Char('?'));
        for _ in 0..16 {
            app.overlays.tick();
        }
        assert!(!app.overlays.has_help(), "再按 ? 应收起(toggle)");
        Ok(())
    }

    /// 集成:queue 开着按 `?` → help 叠其上;back 只关 help,queue 仍在。
    #[test]
    fn help_stacks_on_queue_back_closes_top_only() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.overlays.queue_sel(), Some(0));
        press(&mut app, KeyCode::Char('?'));
        assert!(app.overlays.has_help(), "queue 上按 ? 应叠开 help");
        press(&mut app, KeyCode::Esc);
        for _ in 0..16 {
            app.overlays.tick();
        }
        assert!(!app.overlays.has_help(), "back 关掉栈顶 help");
        assert_eq!(app.overlays.queue_sel(), Some(0), "queue 不受牵连");
        Ok(())
    }

    /// 集成:help 开着,列表导航键被 help 吞掉滚表,不落到底下的 Library 光标。
    #[test]
    fn help_consumes_navigation_keys() -> color_eyre::Result<()> {
        let mut app = app_with_library(10, /*sel_track*/ 4)?;
        press(&mut app, KeyCode::Char('?'));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('G'));
        assert_eq!(
            app.state.browse.nav.track.sel(),
            4,
            "导航键应被 help 吞掉,Library 光标不动"
        );
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

    /// 退出补记:还停在 Library 内走 q→y 退出,光标位置在转场起点记入记忆表
    /// (否则没经过「返回」的位置会随退出丢失)。
    #[test]
    fn quit_records_track_position_from_library() -> color_eyre::Result<()> {
        use mineral_model::{PlaylistId, SourceKind};

        let mut app = app_with_library(10, /*sel_track*/ 4)?;
        press(&mut app, KeyCode::Char('q'));
        press(&mut app, KeyCode::Char('y'));
        let pid = PlaylistId::new(SourceKind::NETEASE, "p1");
        assert_eq!(
            app.state.browse.nav.track_pos.get(&pid).map(|p| p.index),
            Some(4),
            "退出转场起点应补记 Library 光标位置"
        );
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

    /// Shift+Q(硬编码,不可重映射):跳过确认浮层直接进退出收缩动画;
    /// 动画收尾时向 daemon 投递一次 shutdown 请求。
    #[test]
    fn shift_q_quits_with_animation_and_requests_daemon_stop() -> color_eyre::Result<()> {
        let (mut app, shutdowns) = app_with_queue_probed(3, /*current_idx*/ 0)?;
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('Q'),
            KeyModifiers::SHIFT,
        )));
        assert!(
            matches!(&app.transition, Some(t) if t.leaving()),
            "Shift+Q 应直接进入退出(收缩)转场,不弹确认"
        );
        assert!(!app.should_quit, "应先播收缩动画,不立即退");

        for _ in 0..40 {
            if app.transition.is_some() {
                app.tick_transition();
            }
        }
        assert!(app.should_quit, "收缩动画归零后应退出");
        assert_eq!(
            shutdowns.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "退出收尾应恰好投递一次 daemon shutdown"
        );
        Ok(())
    }

    /// 搜索输入态的大写 'Q' 是搜索词,不触发 Shift+Q 退出(硬编码键让位文本输入)。
    #[test]
    fn shift_q_in_search_mode_types_into_query() -> color_eyre::Result<()> {
        let (mut app, shutdowns) = {
            let (mut app, shutdowns) = app_with_queue_probed(3, /*current_idx*/ 0)?;
            app.state
                .browse
                .view
                .switch_to(crate::runtime::state::View::Library);
            (app, shutdowns)
        };
        press(&mut app, KeyCode::Char('/'));
        assert!(app.state.browse.active_search().typing, "前置:已进搜索态");

        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('Q'),
            KeyModifiers::SHIFT,
        )));
        assert_eq!(
            app.state.browse.active_search().query(),
            "Q",
            "大写 Q 应进搜索词"
        );
        assert!(app.transition.is_none(), "搜索态不该触发退出转场");
        assert!(!app.should_quit);
        assert_eq!(
            shutdowns.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "不该投递 daemon shutdown"
        );
        Ok(())
    }

    /// 确认浮层开着时 Shift+Q 仍然生效(压过浮层的 y/n 等待),直接进退出转场。
    #[test]
    fn shift_q_overrides_confirm_overlay() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        press(&mut app, KeyCode::Char('q')); // 开退出确认浮层
        app.handle_event(&Event::Key(KeyEvent::new(
            KeyCode::Char('Q'),
            KeyModifiers::SHIFT,
        )));
        assert!(
            matches!(&app.transition, Some(t) if t.leaving()),
            "浮层开着 Shift+Q 也应直接进入退出转场"
        );
        Ok(())
    }

    /// 普通退出(q → y)不投递 daemon shutdown —— 杀 daemon 只属于 Shift+Q 路径
    /// (Auto 模式自有 `kill_spawned_daemon_on_exit` 旋钮收尾,不经 IPC)。
    #[test]
    fn normal_quit_keeps_daemon_alive() -> color_eyre::Result<()> {
        let (mut app, shutdowns) = app_with_queue_probed(3, /*current_idx*/ 0)?;
        press(&mut app, KeyCode::Char('q'));
        press(&mut app, KeyCode::Char('y'));
        for _ in 0..40 {
            if app.transition.is_some() {
                app.tick_transition();
            }
        }
        assert!(app.should_quit, "前置:正常退出完成");
        assert_eq!(
            shutdowns.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "正常退出不该投递 daemon shutdown"
        );
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
            .map(|entry| entry.data.song.id.clone())
            .ok_or_else(|| color_eyre::eyre::eyre!("fixture 没有曲目"))?;

        // 初始 loved = false。
        assert!(
            !app.state
                .library
                .liked_ids
                .get(&SourceKind::NETEASE)
                .is_some_and(|s| s.contains(&song_id)),
            "初始不应在 liked_ids 里"
        );

        // 按 f → 乐观翻转成 loved。
        press(&mut app, KeyCode::Char('f'));
        assert!(
            app.state
                .library
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
        assert!(
            loved_after_first,
            "第一次按 f 后 PlaylistEntryView.loved 应为 true"
        );

        // 再按 f → 翻转回 not loved。
        press(&mut app, KeyCode::Char('f'));
        assert!(
            !app.state
                .library
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
            "第二次按 f 后 PlaylistEntryView.loved 应为 false"
        );

        Ok(())
    }

    /// 音量键按 `volume_step` 从当前音量算出目标发给 daemon(`+`/`=` 加,`-`/`_` 减,
    /// 钳 0..=100);本地不乐观回写,屏幕数值只认 daemon 推送。
    ///
    /// seek 只发 server 命令,本地 position 无回显。
    #[test]
    fn volume_and_seek_via_keymap() -> color_eyre::Result<()> {
        let (mut app, volumes) = app_with_queue_volume_probed(1, /*current_idx*/ 0)?;

        // 每次按键前把当前音量置成 daemon 已确认值(真实路径下由每帧同步灌入)。
        for (key, base, expected) in [
            (KeyCode::Char('+'), 50_u8, 55_u8),
            (KeyCode::Char('='), 55, 60),
            (KeyCode::Char('-'), 60, 55),
            (KeyCode::Char('_'), 55, 50),
            (KeyCode::Char('+'), 100, 100),
            (KeyCode::Char('-'), 0, 0),
        ] {
            app.state.playback.volume_pct = base;
            press(&mut app, key);
            assert_eq!(
                volumes.lock().ok().and_then(|sent| sent.last().copied()),
                Some(expected),
                "{key:?} 在 {base}% 上应发 {expected}%"
            );
            assert_eq!(app.state.playback.volume_pct, base, "本地不乐观回写");
        }

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

    /// `d` 按视图分流下载意图(Playlists→歌单 / Library→单曲),TestClient no-op:
    /// 断不 panic、选中与视图不变(不验 Client 调用细节)。
    #[test]
    fn d_downloads_selection_by_view() -> color_eyre::Result<()> {
        let mut app = app_with_library(3, /*sel_track*/ 1)?;
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(app.state.browse.nav.track.sel(), 1, "Library d 不动选中");
        assert_eq!(
            app.state.browse.view,
            crate::runtime::state::View::Library,
            "Library d 不切视图"
        );

        let mut app = app_with_library(3, /*sel_track*/ 0)?;
        app.state
            .browse
            .view
            .switch_to(crate::runtime::state::View::Playlists);
        press(&mut app, KeyCode::Char('d'));
        assert_eq!(
            app.state.browse.nav.playlist.sel(),
            0,
            "Playlists d 不动选中"
        );
        assert_eq!(
            app.state.browse.view,
            crate::runtime::state::View::Playlists,
            "Playlists d 不切视图"
        );
        Ok(())
    }

    /// `z` 进/退全屏(toggle):进场目标置满(非 leaving),再按退场目标归零(leaving)。
    #[test]
    fn z_toggles_fullscreen() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        assert!(!app.state.browse.fullscreen.on(), "初始非全屏");

        press(&mut app, KeyCode::Char('z'));
        assert!(
            app.state.browse.fullscreen.on(),
            "z 进全屏(开关 + 形变目标合一)"
        );

        press(&mut app, KeyCode::Char('z'));
        assert!(!app.state.browse.fullscreen.on(), "再按 z 退全屏");
        Ok(())
    }

    /// 浏览态 `s` 进 Search 布局态;布局内退出走 Esc(`s` 已是输入字符,见 token prompt)。
    #[test]
    fn s_opens_search_layout_esc_exits() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        assert!(!app.state.channel_search.active.on(), "初始非 search 布局");

        press(&mut app, KeyCode::Char('s'));
        assert!(app.state.channel_search.active.on(), "s 进 search 布局");

        press(&mut app, KeyCode::Esc);
        assert!(!app.state.channel_search.active.on(), "Esc 退 search 布局");
        Ok(())
    }

    /// 搜索面板上的 s 退场后再按 s，反向沿用同一焦点和进度。
    #[test]
    fn reversing_search_exit_keeps_panel_focus() -> color_eyre::Result<()> {
        use crate::runtime::state::SearchFocus;

        for focus in [SearchFocus::Results, SearchFocus::Detail] {
            let (mut app, _) =
                crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
            app.state.channel_search.set_focus(focus);
            app.state.channel_search.active.retempo(8);
            press(&mut app, KeyCode::Char('s'));
            assert!(!app.state.channel_search.active.on(), "面板上的 s 启动退场");
            for _ in 0..2 {
                app.state.channel_search.active.tick();
            }
            let progress = app.state.channel_search.active.raw();
            assert!(
                !app.state.channel_search.active.at_min(),
                "前置：仍在退场途中"
            );
            press(&mut app, KeyCode::Char('s'));
            assert!(app.state.channel_search.active.on(), "再次按 s 反向进入");
            assert_eq!(app.state.channel_search.active.raw(), progress);
            assert_eq!(
                app.state.channel_search.focus, focus,
                "反向不能重置搜索焦点"
            );
        }
        Ok(())
    }

    /// 互斥:全屏态按 `s` 无效(不进 search 布局)。
    #[test]
    fn fullscreen_blocks_open_search() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        press(&mut app, KeyCode::Char('z'));
        assert!(app.state.browse.fullscreen.on(), "前置:已进全屏");

        press(&mut app, KeyCode::Char('s'));
        assert!(!app.state.channel_search.active.on(), "全屏态 s 无效");
        Ok(())
    }

    /// 互斥:search 布局态按 `z` 无效(不进全屏)。
    #[test]
    fn search_blocks_toggle_fullscreen() -> color_eyre::Result<()> {
        let mut app = app_with_queue(3, /*current_idx*/ 0)?;
        press(&mut app, KeyCode::Char('s'));
        assert!(
            app.state.channel_search.active.on(),
            "前置:已进 search 布局"
        );

        press(&mut app, KeyCode::Char('z'));
        assert!(!app.state.browse.fullscreen.on(), "search 态 z 无效");
        Ok(())
    }

    /// Search 布局态(prompt 焦点)字符键进当前会话 query。
    #[test]
    fn typing_in_search_appends_to_query() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        press(&mut app, KeyCode::Char('h'));
        press(&mut app, KeyCode::Char('i'));
        let session = app
            .state
            .channel_search
            .current()
            .ok_or_else(|| eyre!("应有当前会话"))?;
        assert_eq!(session.query(), "hi", "字符进当前会话 query");
        Ok(())
    }

    /// Search 布局态退格删 query 末字。
    #[test]
    fn backspace_in_search_pops_query() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        for c in "hi".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Backspace);
        let session = app
            .state
            .channel_search
            .current()
            .ok_or_else(|| eyre!("应有当前会话"))?;
        assert_eq!(session.query(), "h", "退格删一字");
        Ok(())
    }

    /// Enter 提交 `ChannelFetchKind::Search`(源/kind/query/page 正确),焦点落结果列。
    #[test]
    fn enter_submits_search_task_and_focuses_results() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        use mineral_channel_core::Page;
        use mineral_task::{ChannelFetchKind, TaskKind};

        use crate::runtime::state::SearchFocus;

        let (mut app, submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        for c in "hello".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        let tasks = submitted.lock().map_err(|e| eyre!("探针锁中毒: {e}"))?;
        let submitted_search = tasks.iter().find_map(|k| match k {
            TaskKind::ChannelFetch(ChannelFetchKind::Search {
                source,
                kind,
                query,
                page,
            }) => Some((*source, *kind, query.clone(), *page)),
            _ => None,
        });
        assert_eq!(
            submitted_search,
            Some((
                SourceKind::NETEASE,
                SearchKind::Song,
                "hello".to_owned(),
                Page::new(/*offset*/ 0, /*limit*/ 30)
            )),
            "Enter 提交首页 Search 任务"
        );
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Results,
            "提交后焦点落结果列"
        );
        Ok(())
    }

    /// 造一个已提交并落了 `n` 首结果、焦点在结果列的 App(j/k 导航测试前置)。
    fn app_with_results(n: usize) -> color_eyre::Result<App> {
        use mineral_channel_core::Page;
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::test_support::endserenading;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Enter);
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "x".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Songs(endserenading(n)),
            has_more: None,
        });
        Ok(app)
    }

    /// 结果列导航走 config 绑定(非写死键):`J`(move_down_big)经 keymap 大步下移、钳末行。
    /// 旧写死实现只认 j/k,`J` 无效——本测试证明导航键已 config 化。
    #[test]
    fn results_focus_big_jump_is_config_driven() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        let mut app = app_with_results(4)?;
        press(&mut app, KeyCode::Char('J'));
        let sel = app
            .state
            .channel_search
            .active_results()
            .ok_or_else(|| eyre!("应有结果桶"))?
            .sel();
        assert_eq!(sel, 3, "J 大步下移经 config 绑定生效(4 首钳到末行)");
        Ok(())
    }

    /// 结果列焦点:`j`/`k` 在结果间移光标(钳末行 / 首行)。
    #[test]
    fn results_focus_jk_moves_selection() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        let mut app = app_with_results(4)?;
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        let sel = app
            .state
            .channel_search
            .active_results()
            .ok_or_else(|| eyre!("应有结果桶"))?
            .sel();
        assert_eq!(sel, 2, "j 下移两次");
        press(&mut app, KeyCode::Char('k'));
        let sel = app
            .state
            .channel_search
            .active_results()
            .ok_or_else(|| eyre!("应有结果桶"))?
            .sel();
        assert_eq!(sel, 1, "k 上移一次");
        Ok(())
    }

    /// 结果列光标进入预取半径后请求下一页；收包前连续移动只提交一次，页大小沿用首页。
    #[test]
    fn results_prefetch_dispatches_next_page_near_bottom() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        use mineral_channel_core::Page;
        use mineral_task::{ChannelFetchKind, SearchPayload, TaskEvent, TaskKind};

        use crate::test_support::endserenading;

        let (mut app, submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        press(&mut app, KeyCode::Char('x'));
        press(&mut app, KeyCode::Enter);
        // 满页(loaded == limit)→ 未榨干、next_offset = 10。
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Song,
            query: "x".to_owned(),
            page: Page::new(/*offset*/ 0, /*limit*/ 10),
            payload: SearchPayload::Songs(endserenading(10)),
            has_more: None,
        });

        let requested_pages = |tasks: &[TaskKind]| {
            tasks
                .iter()
                .filter_map(|k| match k {
                    TaskKind::ChannelFetch(ChannelFetchKind::Search { page, .. })
                        if page.offset > 0 =>
                    {
                        Some(*page)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>()
        };

        // 顶部、未移动:不预取下一页。
        {
            let tasks = submitted.lock().map_err(|e| eyre!("探针锁中毒: {e}"))?;
            assert!(requested_pages(&tasks).is_empty(), "顶部不预取下一页");
        }

        // 光标连按下移到底 → 进入距底范围 → 预取 offset = next_offset(10)的下一页。
        for _ in 0..9 {
            press(&mut app, KeyCode::Char('j'));
        }
        let tasks = submitted.lock().map_err(|e| eyre!("探针锁中毒: {e}"))?;
        assert_eq!(
            requested_pages(&tasks),
            vec![Page::new(10, 10)],
            "光标近底只请求一次下一页，offset 和 limit 沿用首页分页"
        );
        Ok(())
    }

    /// 结果列焦点 Esc 回 prompt(不退出布局态)。
    #[test]
    fn results_focus_esc_returns_to_prompt() -> color_eyre::Result<()> {
        use crate::runtime::state::SearchFocus;

        let mut app = app_with_results(4)?;
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Results,
            "前置:焦点在结果列"
        );
        press(&mut app, KeyCode::Esc);
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Prompt,
            "Esc 回 prompt"
        );
        assert!(app.state.channel_search.active.on(), "仍在 search 布局");
        Ok(())
    }

    /// prompt 焦点 Tab:**只切到上次面板(默认 results)、不提交搜索**——区别于 Enter(搜索+切)。
    #[test]
    fn prompt_tab_switches_to_last_panel_without_search() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;

        use crate::runtime::state::SearchFocus;

        let (mut app, submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        for c in "hi".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Results,
            "Tab 切到上次面板(默认 results)"
        );
        let tasks = submitted.lock().map_err(|e| eyre!("探针锁中毒: {e}"))?;
        assert!(tasks.is_empty(), "Tab 只切不搜:无 Search 任务提交");
        Ok(())
    }

    /// results↔detail 空间导航:drill(Ctrl+L)进 detail、back(Ctrl+H)回 results。
    /// song 结果的 `l` 已是「直接播放」,故进 detail 走 drill 这一对(与全 app 的 back 同源)。
    #[test]
    fn results_detail_drill_navigation() -> color_eyre::Result<()> {
        use crate::runtime::state::SearchFocus;

        let mut app = app_with_results(4)?;
        press_ctrl(&mut app, KeyCode::Char('l'));
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Detail,
            "Ctrl+L drill 进 detail"
        );
        press_ctrl(&mut app, KeyCode::Char('h'));
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Results,
            "Ctrl+H 回 results"
        );
        Ok(())
    }

    /// detail 焦点下 `move_*` 是 no-op,不得移动 results 光标。
    #[test]
    fn detail_focus_move_does_not_touch_results() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;

        use crate::runtime::state::SearchFocus;

        let mut app = app_with_results(4)?;
        press_ctrl(&mut app, KeyCode::Char('l')); // results → detail（song 结果走 drill 进入）
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Detail,
            "前置:焦点在 detail"
        );
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        let sel = app
            .state
            .channel_search
            .active_results()
            .ok_or_else(|| eyre!("应有结果桶"))?
            .sel();
        assert_eq!(sel, 0, "detail 焦点下 j/k 不动 results 光标");
        Ok(())
    }

    /// 空 query 按 left → 焦点跨到 kind chip 段,下拉自动展开。
    #[test]
    fn left_from_empty_query_opens_kind_chip() -> color_eyre::Result<()> {
        use crate::runtime::state::PromptSegment;
        let (mut app, _submitted) = crate::test_support::app_with_channel_search_probed(vec![
            SearchKind::Song,
            SearchKind::Album,
        ])?;
        press(&mut app, KeyCode::Left);
        assert_eq!(
            app.state.channel_search.prompt_seg(),
            PromptSegment::Kind,
            "词首 left 跨到 kind chip"
        );
        assert!(
            app.state.channel_search.seg_open(),
            "focus 到 chip 即展开下拉"
        );
        Ok(())
    }

    /// 从 kind chip 再 left → 跨到 source chip 段(最左)。
    #[test]
    fn left_from_kind_chip_reaches_source_chip() -> color_eyre::Result<()> {
        use crate::runtime::state::PromptSegment;
        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        press(&mut app, KeyCode::Left); // query → kind chip
        press(&mut app, KeyCode::Left); // kind → source chip
        assert_eq!(
            app.state.channel_search.prompt_seg(),
            PromptSegment::Source,
            "kind chip 再 left 到 source chip"
        );
        assert!(app.state.channel_search.seg_open(), "source chip 下拉展开");
        Ok(())
    }

    /// `@` / `$` 退役为普通字符:在 query 段直接进 query,不触发任何菜单。
    #[test]
    fn at_and_dollar_are_plain_query_chars() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;
        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        press(&mut app, KeyCode::Char('@'));
        press(&mut app, KeyCode::Char('$'));
        assert_eq!(app.overlays.len(), 0, "@/$ 不再弹菜单");
        let query = app
            .state
            .channel_search
            .current()
            .ok_or_else(|| eyre!("应有会话"))?
            .query()
            .to_owned();
        assert_eq!(query, "@$", "@/$ 当普通字符进 query");
        Ok(())
    }

    /// kind chip 选中无缓存类型 + query 非空 → Enter 确认时用当前 query 自动搜该类型。
    #[test]
    fn confirm_kind_auto_searches_empty_bucket() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;

        use mineral_task::{ChannelFetchKind, TaskKind};

        let (mut app, submitted) = crate::test_support::app_with_channel_search_probed(vec![
            SearchKind::Song,
            SearchKind::Album,
        ])?;
        press(&mut app, KeyCode::Char('q')); // query = "q"
        press(&mut app, KeyCode::Left); // 光标到词首
        press(&mut app, KeyCode::Left); // 跨到 kind chip(下拉开,高亮当前 Song)
        press(&mut app, KeyCode::Down); // 高亮 Album
        press(&mut app, KeyCode::Enter); // 确认 → 自动搜 Album
        let tasks = submitted.lock().map_err(|e| eyre!("探针锁中毒: {e}"))?;
        assert!(
            tasks.iter().any(|t| matches!(
                t,
                TaskKind::ChannelFetch(ChannelFetchKind::Search {
                    kind: SearchKind::Album,
                    ..
                })
            )),
            "确认无缓存 kind + query 非空 → 自动搜 Album"
        );
        Ok(())
    }

    /// 造一张测试专辑。
    fn test_album(raw: &str) -> mineral_model::Album {
        mineral_model::Album::builder()
            .id(mineral_model::AlbumId::new(SourceKind::NETEASE, raw))
            .name(format!("album {raw}"))
            .build()
    }

    /// artist detail:[ ] 切专辑区、activate 专辑下钻一帧、Esc 弹回 root。
    #[test]
    fn artist_detail_section_drill_and_back() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::{Artist, ArtistId};
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::{ArtistSection, SearchFocus};

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Artist])?;
        let artist = |songs: Vec<mineral_model::Song>| {
            Artist::builder()
                .id(ArtistId::new(SourceKind::NETEASE, "ar1"))
                .name("Artist".to_owned())
                .songs(songs)
                .build()
        };
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Artist,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Artists(vec![artist(Vec::new())]),
            has_more: None,
        });
        let id = ArtistId::new(SourceKind::NETEASE, "ar1");
        app.state.apply(&TaskEvent::ArtistDetailFetched {
            id: id.clone(),
            artist: Box::new(artist(crate::test_support::endserenading(2))),
        });
        app.state.apply(&TaskEvent::ArtistAlbumsFetched {
            id,
            page: Page::default(),
            albums: vec![test_album("a1"), test_album("a2")],
            has_more: None,
        });
        app.state.channel_search.set_focus(SearchFocus::Results);
        press(&mut app, KeyCode::Char('l')); // results → detail
        assert_eq!(app.state.channel_search.focus, SearchFocus::Detail);
        press(&mut app, KeyCode::Char('[')); // Hot → Albums
        let section = app
            .state
            .channel_search
            .active_results()
            .and_then(|kr| kr.detail.current())
            .map(|f| f.section);
        assert_eq!(section, Some(ArtistSection::Albums), "[ 切到专辑区");
        press(&mut app, KeyCode::Char('l')); // activate 专辑 → 下钻
        assert_eq!(
            app.state
                .channel_search
                .active_results()
                .map(|kr| kr.detail.depth()),
            Some(1),
            "专辑区 activate 下钻一帧"
        );
        press(&mut app, KeyCode::Esc); // 弹回 root
        assert_eq!(
            app.state
                .channel_search
                .active_results()
                .map(|kr| kr.detail.depth()),
            Some(0),
            "Esc 弹回 root"
        );
        Ok(())
    }

    /// detail 焦点 j/k 滚当前区列表(钳列表长度)。
    #[test]
    fn detail_jk_scrolls_list() -> color_eyre::Result<()> {
        use mineral_channel_core::Page;
        use mineral_model::AlbumId;
        use mineral_task::{SearchPayload, TaskEvent};

        use crate::runtime::state::SearchFocus;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Album])?;
        if let Some(s) = app.state.channel_search.current_mut() {
            s.set_query("q");
        }
        app.state.apply(&TaskEvent::SearchResults {
            source: SourceKind::NETEASE,
            kind: SearchKind::Album,
            query: "q".to_owned(),
            page: Page::default(),
            payload: SearchPayload::Albums(vec![test_album("al1")]),
            has_more: None,
        });
        app.state.apply(&TaskEvent::AlbumDetailFetched {
            id: AlbumId::new(SourceKind::NETEASE, "al1"),
            album: Box::new(
                mineral_model::Album::builder()
                    .id(AlbumId::new(SourceKind::NETEASE, "al1"))
                    .name("al1".to_owned())
                    .tracks(mineral_model::AlbumTrack::enumerate(
                        crate::test_support::endserenading(4),
                    ))
                    .build(),
            ),
        });
        app.state.channel_search.set_focus(SearchFocus::Detail);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('j'));
        assert_eq!(
            app.state
                .channel_search
                .active_results()
                .and_then(|kr| kr.detail.current())
                .map(|f| f.list().sel()),
            Some(2),
            "detail j 滚两次到第 3 行(同专辑曲目)"
        );
        Ok(())
    }

    /// 搜索框(prompt 焦点)是模态文本输入:q / Q 都进 query——不弹退出框、不触发强退转场。
    #[test]
    fn search_box_q_and_capital_q_are_query_chars() -> color_eyre::Result<()> {
        use color_eyre::eyre::eyre;

        let (mut app, _submitted) =
            crate::test_support::app_with_channel_search_probed(vec![SearchKind::Song])?;
        press(&mut app, KeyCode::Char('Q'));
        press(&mut app, KeyCode::Char('q'));
        assert!(app.transition.is_none(), "搜索框 Q 不触发强退收缩转场");
        assert_eq!(app.overlays.len(), 0, "搜索框 q 不弹退出确认框");
        let query = app
            .state
            .channel_search
            .current()
            .ok_or_else(|| eyre!("应有会话"))?
            .query()
            .to_owned();
        assert_eq!(query, "Qq", "q/Q 都进 query");
        Ok(())
    }

    /// 搜索界面(results 焦点,非搜索框)非导航键回落全局:q 弹退出确认框(证明 transport 等
    /// 全局键走同一回落路径生效)。
    #[test]
    fn search_view_q_falls_through_to_quit_confirm() -> color_eyre::Result<()> {
        let mut app = app_with_results(4)?;
        let before = app.overlays.len();
        press(&mut app, KeyCode::Char('q'));
        assert_eq!(
            app.overlays.len(),
            before + 1,
            "results 焦点 q 回落全局 → 弹退出确认框"
        );
        assert!(!app.should_quit, "q 是确认框,不是直接退出");
        Ok(())
    }

    /// 搜索界面(results 焦点)Shift+Q 逃生口照常生效:触发强退收缩转场(只有搜索框才让位)。
    #[test]
    fn search_view_capital_q_force_quits() -> color_eyre::Result<()> {
        let mut app = app_with_results(4)?;
        press(&mut app, KeyCode::Char('Q'));
        assert!(app.transition.is_some(), "results 焦点 Q 触发强退收缩转场");
        Ok(())
    }

    /// 面板 Tab 回 prompt;Esc 走退一级链(detail→results→prompt);prompt 态 Tab 记住「上次面板」(detail)。
    #[test]
    fn panels_tab_esc_return_to_prompt_remembering_last() -> color_eyre::Result<()> {
        use crate::runtime::state::SearchFocus;

        let mut app = app_with_results(4)?;
        press_ctrl(&mut app, KeyCode::Char('l')); // results → detail（song 结果走 drill 进入）
        press(&mut app, KeyCode::Tab); // detail → prompt
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Prompt,
            "detail Tab 回 prompt"
        );
        press(&mut app, KeyCode::Tab); // prompt → 上次面板 = detail
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Detail,
            "Tab 回上次面板=detail(记住位置)"
        );
        press(&mut app, KeyCode::Esc); // detail → results(Esc 链退一级)
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Results,
            "detail Esc 退一级到 results(Esc 链)"
        );
        press(&mut app, KeyCode::Esc); // results → prompt
        assert_eq!(
            app.state.channel_search.focus,
            SearchFocus::Prompt,
            "results Esc 回 prompt"
        );
        assert!(app.state.channel_search.active.on(), "Esc 链不退出布局态");
        Ok(())
    }

    /// Search 面板按键只操作当前页面的选中实体、滚动和查询输入。
    mod search_panel_routing {
        use std::sync::Arc;

        use color_eyre::eyre::eyre;
        use mineral_channel_core::Page;
        use mineral_model::{Album, AlbumId, Artist, ArtistId, Playlist, PlaylistId, Song, SongId};
        use mineral_task::{ChannelFetchKind, SearchPayload, TaskEvent, TaskKind};

        use super::{App, KeyCode, SearchKind, SourceKind, press, press_ctrl};
        use crate::runtime::state::{EntityRef, PromptSegment, SearchFocus, View};
        use crate::test_support::{
            TestClient, app_with_channel_search_probed, app_with_long_library, song,
        };

        /// 构造与后台 Library 不同的搜索曲目，避免选错目标被同一 ID 掩盖。
        fn search_songs(len: usize) -> Vec<Song> {
            (0..len).map(|i| song(&format!("search-{i}"))).collect()
        }

        /// 在保留过滤词和列表位置的 Library 上提交搜索，并注入结果及请求探针。
        fn search_over_browse(
            kind: SearchKind,
            payload: SearchPayload,
            has_more: bool,
        ) -> color_eyre::Result<(App, Arc<TestClient>)> {
            let (mut app, _) = app_with_channel_search_probed(vec![kind])?;
            let browse = app_with_long_library(64, 22)?;
            app.state.browse = browse.state.browse;
            app.state.library = browse.state.library;
            app.state.browse.active_search_mut().set_query("Track");
            app.state.browse.nav.track.place(22, 4);
            let client = Arc::new(TestClient::default());
            app.client = client.clone();
            for c in "remote".chars() {
                press(&mut app, KeyCode::Char(c));
            }
            press(&mut app, KeyCode::Enter);
            app.state.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind,
                query: "remote".to_owned(),
                page: Page::default(),
                payload,
                has_more: Some(has_more),
            });
            client
                .submitted
                .lock()
                .map_err(|e| eyre!("任务探针锁中毒: {e}"))?
                .clear();
            assert_browse_unchanged(&app);
            Ok((app, client))
        }

        /// 后台 Browse 保留视图、过滤输入、两个列表的位置和所有歌曲的喜欢态。
        fn assert_browse_unchanged(app: &App) {
            assert_eq!(app.state.browse.view.current(), View::Library);
            assert_eq!(app.state.browse.active_search().query(), "Track");
            assert!(!app.state.browse.active_search().typing);
            assert_eq!(app.state.browse.nav.playlist.sel(), 0);
            assert_eq!(app.state.browse.nav.playlist.scroll_target(), 0);
            assert_eq!(app.state.browse.nav.track.sel(), 22);
            assert_eq!(app.state.browse.nav.track.scroll_target(), 18);
            assert!(
                app.state
                    .library
                    .tracks
                    .values()
                    .flat_map(|tracks| tracks.iter())
                    .all(|entry| !entry.loved)
            );
        }

        /// 读取实际发给后端的喜欢态持久化请求目标。
        fn love_targets(client: &TestClient) -> color_eyre::Result<Vec<SongId>> {
            Ok(client
                .love_requests
                .lock()
                .map_err(|e| eyre!("喜欢探针锁中毒: {e}"))?
                .iter()
                .map(|song| song.id.clone())
                .collect())
        }

        /// 热更为可区分的滚动步长，验证按键处理现读配置。
        fn configure_scroll(app: &mut App) -> color_eyre::Result<()> {
            let tree = mineral_config::merge_tree(
                mineral_config::default_tree()?,
                serde_json::json!({ "tui": { "behavior": {
                    "line_scroll_rows": 3,
                    "page_scroll_rows": 11,
                    "search_prefetch_rows": 2
                } } }),
            );
            app.apply_pushed_config(mineral_protocol::BusValue::from_json(tree));
            assert_eq!(*app.state.cfg.tui().behavior().line_scroll_rows(), 3);
            assert_eq!(*app.state.cfg.tui().behavior().page_scroll_rows(), 11);
            Ok(())
        }

        /// 构造搜索结果中的专辑壳，详情通过独立回包注入。
        fn album() -> Album {
            Album::builder()
                .id(AlbumId::new(SourceKind::NETEASE, "search-album"))
                .name("Search Album".to_owned())
                .build()
        }

        /// Results 的 f 持久化当前曲目，两次按键乐观翻转喜欢态。
        #[test]
        fn results_f_persists_selected_song() -> color_eyre::Result<()> {
            let songs = search_songs(3);
            let selected = songs
                .get(1)
                .ok_or_else(|| eyre!("缺少第二首搜索曲目"))?
                .clone();
            let (mut app, client) =
                search_over_browse(SearchKind::Song, SearchPayload::Songs(songs), false)?;
            press(&mut app, KeyCode::Char('j'));
            press(&mut app, KeyCode::Char('f'));
            assert_eq!(love_targets(&client)?, vec![selected.id.clone()]);
            assert!(app.state.is_liked(&selected));
            assert_browse_unchanged(&app);

            press(&mut app, KeyCode::Char('f'));
            assert_eq!(
                love_targets(&client)?,
                vec![selected.id.clone(), selected.id.clone()]
            );
            assert!(!app.state.is_liked(&selected));
            assert_browse_unchanged(&app);
            Ok(())
        }

        /// Detail 未加载时 f 不操作；专辑详情到货后，f 持久化详情列表当前曲目。
        #[test]
        fn detail_f_persists_selected_track_after_loading() -> color_eyre::Result<()> {
            let album = album();
            let (mut app, client) = search_over_browse(
                SearchKind::Album,
                SearchPayload::Albums(vec![album.clone()]),
                false,
            )?;
            press(&mut app, KeyCode::Enter);
            assert_eq!(app.state.channel_search.focus, SearchFocus::Detail);
            press(&mut app, KeyCode::Char('f'));
            assert!(love_targets(&client)?.is_empty());
            assert_browse_unchanged(&app);

            let songs = search_songs(3);
            let selected = songs
                .get(1)
                .ok_or_else(|| eyre!("缺少第二首专辑曲目"))?
                .clone();
            let mut detailed_album = album;
            detailed_album.tracks = mineral_model::AlbumTrack::enumerate(songs);
            app.state.apply(&TaskEvent::AlbumDetailFetched {
                id: detailed_album.id.clone(),
                album: Box::new(detailed_album),
            });
            press(&mut app, KeyCode::Char('j'));
            press(&mut app, KeyCode::Char('f'));
            assert_eq!(love_targets(&client)?, vec![selected.id.clone()]);
            assert!(app.state.is_liked(&selected));
            assert_browse_unchanged(&app);
            Ok(())
        }

        /// Results 的专辑、艺人、歌单和空结果没有单曲喜欢语义。
        #[test]
        fn results_f_ignores_containers_and_empty_rows() -> color_eyre::Result<()> {
            let artist = Artist::builder()
                .id(ArtistId::new(SourceKind::NETEASE, "search-artist"))
                .name("Search Artist".to_owned())
                .build();
            let playlist = Playlist::builder()
                .id(PlaylistId::new(SourceKind::NETEASE, "search-playlist"))
                .name("Search Playlist".to_owned())
                .build();
            for (kind, payload) in [
                (SearchKind::Album, SearchPayload::Albums(vec![album()])),
                (SearchKind::Artist, SearchPayload::Artists(vec![artist])),
                (
                    SearchKind::Playlist,
                    SearchPayload::Playlists(vec![playlist]),
                ),
                (SearchKind::Song, SearchPayload::Songs(Vec::new())),
            ] {
                let (mut app, client) = search_over_browse(kind, payload, false)?;
                press(&mut app, KeyCode::Char('f'));
                assert!(
                    love_targets(&client)?.is_empty(),
                    "{kind:?} 不应发送喜欢请求"
                );
                assert!(
                    app.state
                        .library
                        .liked_ids
                        .values()
                        .all(rustc_hash::FxHashSet::is_empty)
                );
                assert_browse_unchanged(&app);
            }
            Ok(())
        }

        /// 艺人 Detail 的热门曲可喜欢；切到专辑行或曲目光标越界后 f 不操作。
        #[test]
        fn artist_detail_f_uses_only_song_rows() -> color_eyre::Result<()> {
            let songs = search_songs(2);
            let selected = songs
                .get(1)
                .ok_or_else(|| eyre!("缺少第二首热门曲"))?
                .clone();
            let artist = Artist::builder()
                .id(ArtistId::new(SourceKind::NETEASE, "search-artist"))
                .name("Search Artist".to_owned())
                .songs(songs)
                .build();
            let (mut app, client) = search_over_browse(
                SearchKind::Artist,
                SearchPayload::Artists(vec![artist.clone()]),
                false,
            )?;
            app.state.apply(&TaskEvent::ArtistDetailFetched {
                id: artist.id.clone(),
                artist: Box::new(artist.clone()),
            });
            app.state.apply(&TaskEvent::ArtistAlbumsFetched {
                id: artist.id,
                page: Page::default(),
                albums: vec![album()],
                has_more: None,
            });
            press(&mut app, KeyCode::Enter);
            press(&mut app, KeyCode::Char('j'));
            press(&mut app, KeyCode::Char('f'));
            assert_eq!(love_targets(&client)?, vec![selected.id.clone()]);

            app.state
                .channel_search
                .active_results_mut()
                .and_then(|results| results.detail.current_mut())
                .ok_or_else(|| eyre!("缺少艺人详情"))?
                .list_mut()
                .set_sel(2);
            press(&mut app, KeyCode::Char('f'));
            press(&mut app, KeyCode::Char(']'));
            press(&mut app, KeyCode::Char('f'));
            assert_eq!(love_targets(&client)?, vec![selected.id.clone()]);
            assert!(app.state.is_liked(&selected));
            assert_browse_unchanged(&app);
            Ok(())
        }

        /// 四个滚动键使用热更步长移动 Results，远离底部时不预取，也不滚后台列表。
        #[test]
        fn results_scroll_uses_configured_steps() -> color_eyre::Result<()> {
            let (mut app, client) = search_over_browse(
                SearchKind::Song,
                SearchPayload::Songs(search_songs(40)),
                true,
            )?;
            configure_scroll(&mut app)?;
            for (key, selected) in [('d', 3), ('u', 0), ('f', 11), ('b', 0), ('b', 0)] {
                press_ctrl(&mut app, KeyCode::Char(key));
                assert_eq!(
                    app.state
                        .channel_search
                        .active_results()
                        .ok_or_else(|| eyre!("缺少结果"))?
                        .sel(),
                    selected,
                    "Ctrl-{key}"
                );
                assert_browse_unchanged(&app);
            }
            assert!(
                client
                    .submitted
                    .lock()
                    .map_err(|e| eyre!("任务探针锁中毒: {e}"))?
                    .is_empty()
            );
            Ok(())
        }

        /// Results 滚动近底触发下一页；光标移动复位详情，末行钳制保留下钻栈，榨干后停预取。
        #[test]
        fn results_scroll_preserves_prefetch_and_detail_reset() -> color_eyre::Result<()> {
            let (mut app, client) = search_over_browse(
                SearchKind::Song,
                SearchPayload::Songs(search_songs(40)),
                true,
            )?;
            configure_scroll(&mut app)?;
            let results = app
                .state
                .channel_search
                .active_results_mut()
                .ok_or_else(|| eyre!("缺少结果"))?;
            results.set_sel(36);
            results.detail.push(EntityRef::Album(Box::new(album())), 1);
            press_ctrl(&mut app, KeyCode::Char('d'));
            let results = app
                .state
                .channel_search
                .active_results()
                .ok_or_else(|| eyre!("缺少结果"))?;
            assert_eq!(results.sel(), 39);
            assert_eq!(results.detail.depth(), 0);
            assert!(
                matches!(&results.detail.current().ok_or_else(|| eyre!("缺少详情根帧"))?.entity,
                EntityRef::Song(song) if song.id == SongId::new(SourceKind::NETEASE, "search-39"))
            );
            {
                let submitted = client
                    .submitted
                    .lock()
                    .map_err(|e| eyre!("任务探针锁中毒: {e}"))?;
                assert_eq!(submitted.len(), 1);
                assert!(
                    matches!(submitted.first(), Some(TaskKind::ChannelFetch(ChannelFetchKind::Search {
                    source, kind, query, page
                })) if *source == SourceKind::NETEASE && *kind == SearchKind::Song && query == "remote"
                    && *page == Page::new(Page::default().limit, Page::default().limit))
                );
            }
            let results = app
                .state
                .channel_search
                .active_results_mut()
                .ok_or_else(|| eyre!("缺少结果"))?;
            results.detail.push(EntityRef::Album(Box::new(album())), 1);
            press_ctrl(&mut app, KeyCode::Char('f'));
            let results = app
                .state
                .channel_search
                .active_results()
                .ok_or_else(|| eyre!("缺少结果"))?;
            assert_eq!(results.sel(), 39);
            assert_eq!(results.detail.depth(), 1, "光标未移动时保留下钻栈");
            let requests_before_exhausted = client
                .submitted
                .lock()
                .map_err(|e| eyre!("任务探针锁中毒: {e}"))?
                .len();

            app.state.apply(&TaskEvent::SearchResults {
                source: SourceKind::NETEASE,
                kind: SearchKind::Song,
                query: "remote".to_owned(),
                page: Page::new(Page::default().limit, Page::default().limit),
                payload: SearchPayload::Songs(Vec::new()),
                has_more: Some(false),
            });
            press_ctrl(&mut app, KeyCode::Char('f'));
            assert_eq!(
                client
                    .submitted
                    .lock()
                    .map_err(|e| eyre!("任务探针锁中毒: {e}"))?
                    .len(),
                requests_before_exhausted,
                "榨干后不再提交分页"
            );
            assert_browse_unchanged(&app);
            Ok(())
        }

        /// Detail 的四个滚动键保留滚简介语义，详情曲目和后台 Browse 的位置都不变。
        #[test]
        fn detail_scroll_keeps_list_positions() -> color_eyre::Result<()> {
            let album = album();
            let (mut app, client) = search_over_browse(
                SearchKind::Album,
                SearchPayload::Albums(vec![album.clone()]),
                false,
            )?;
            let mut detailed_album = album;
            detailed_album.description = "line\n".repeat(40);
            detailed_album.tracks = mineral_model::AlbumTrack::enumerate(search_songs(3));
            app.state.apply(&TaskEvent::AlbumDetailFetched {
                id: detailed_album.id.clone(),
                album: Box::new(detailed_album),
            });
            configure_scroll(&mut app)?;
            press(&mut app, KeyCode::Enter);
            for (key, offset) in [('d', 3), ('u', 0), ('f', 11), ('b', 0), ('b', 0)] {
                press_ctrl(&mut app, KeyCode::Char(key));
                let results = app
                    .state
                    .channel_search
                    .active_results()
                    .ok_or_else(|| eyre!("缺少结果"))?;
                let frame = results.detail.current().ok_or_else(|| eyre!("缺少详情"))?;
                assert_eq!(frame.description_scroll().get(), offset, "Ctrl-{key}");
                assert_eq!(frame.list().sel(), 0);
                assert_eq!(results.sel(), 0);
                assert_browse_unchanged(&app);
            }
            assert!(
                client
                    .submitted
                    .lock()
                    .map_err(|e| eyre!("任务探针锁中毒: {e}"))?
                    .is_empty()
            );
            Ok(())
        }

        /// Results 和 Detail 的 / 都回 Query 输入段，保留搜索词及后台 Browse 过滤状态。
        #[test]
        fn slash_returns_to_query_without_changing_browse() -> color_eyre::Result<()> {
            for focus in [SearchFocus::Results, SearchFocus::Detail] {
                let (mut app, client) = search_over_browse(
                    SearchKind::Song,
                    SearchPayload::Songs(search_songs(3)),
                    false,
                )?;
                app.state.channel_search.set_focus(SearchFocus::Prompt);
                app.state
                    .channel_search
                    .set_prompt_seg(PromptSegment::Kind, 0);
                app.state.channel_search.set_focus(focus);
                press(&mut app, KeyCode::Char('/'));
                assert_eq!(app.state.channel_search.focus, SearchFocus::Prompt);
                assert_eq!(
                    app.state.channel_search.prompt_focus(),
                    Some(PromptSegment::Query)
                );
                assert!(!app.state.channel_search.seg_open());
                assert!(app.state.channel_search.active.on());
                assert_eq!(
                    app.state
                        .channel_search
                        .current()
                        .ok_or_else(|| eyre!("缺少搜索会话"))?
                        .query(),
                    "remote"
                );
                assert!(
                    client
                        .submitted
                        .lock()
                        .map_err(|e| eyre!("任务探针锁中毒: {e}"))?
                        .is_empty()
                );
                assert_browse_unchanged(&app);

                press(&mut app, KeyCode::Char('!'));
                assert_eq!(
                    app.state
                        .channel_search
                        .current()
                        .ok_or_else(|| eyre!("缺少搜索会话"))?
                        .query(),
                    "remote!"
                );
                assert_browse_unchanged(&app);
            }
            Ok(())
        }
    }

    /// 全屏态内 `Tab` 仍打开 queue 浮层(浮层是独立层),光标落在在播歌。
    #[test]
    fn fullscreen_tab_still_opens_queue() -> color_eyre::Result<()> {
        let mut app = app_with_queue(4, /*current_idx*/ 1)?;
        press(&mut app, KeyCode::Char('z'));
        assert!(app.state.browse.fullscreen.on());

        press(&mut app, KeyCode::Tab);
        assert_eq!(
            app.overlays.queue_sel(),
            Some(1),
            "全屏内 Tab 仍开 queue,光标在在播歌"
        );
        Ok(())
    }
}

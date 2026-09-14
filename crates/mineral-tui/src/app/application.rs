//! 应用顶层状态与启动构造：配置、图片引擎和后端订阅在此接线。

use std::sync::Arc;
use std::time::Instant;

use ratatui::layout::Position;
use rustc_hash::FxHashMap;

use crate::components::popup::OverlayStack;
use crate::components::toast::download_toast::DownloadNotifier;
use crate::components::toast::notifications::Notifications;
use crate::image::ImageEngine;
use crate::player_actions::PlayMode;
use crate::render::anim::{Transition, ticks16_from_ms};
use crate::render::theme::Theme;
use crate::runtime::backend::Backend;
use crate::runtime::keymap::Keymap;
use crate::runtime::state::AppState;
use crate::runtime::ui::prefs::UiPrefs;
use crate::runtime::window_title::WindowTitle;

/// 应用顶层状态。
pub struct App {
    /// 是否退出主循环。
    pub should_quit: bool,

    /// 本帧的 effective 主题(`Arc` 共享,渲染处只读引用):`theme_base` 经动态
    /// accent 合成的产物,每拍由 `tick_dynamic_accent` 重建;动态主题静止时与 base 相等。
    pub theme: std::sync::Arc<Theme>,

    /// 配置落地的 base 主题(`Theme::from_config` 产物,热更时整体重建)。
    /// 渲染永远读 `theme`,本字段只作动态 accent 合成与回落的基准。
    pub(crate) theme_base: Theme,

    /// 封面驱动的 accent 渐变状态机:目标由 `sync_cover_palette` 按封面身份 diff
    /// 投喂,每拍推进并合成 effective `theme`。
    pub(crate) accent_fade: crate::render::accent::AccentFade,

    /// 全屏氛围背景的调色板渐变状态机:与 accent 共用同一次封面身份 diff 投喂,
    /// 时长独立(`ambient.fade_ms`);渲染在全屏 paint 开头整屏铺 bg。
    pub(crate) ambient: crate::render::ambient::AmbientGradient,

    /// 全屏氛围背景的响度包络:`update_spectrum` 拉到的 PCM 样本每拍喂入,
    /// 场浓度随它呼吸(`ambient.pulse`)。
    pub(crate) ambient_pulse: crate::render::ambient::LoudnessPulse,

    /// 业务状态(视图、选中、playback 镜像、加载缓存等)。
    pub state: AppState,

    /// 键 → 动作绑定表(由配置 keys/behavior 段落地)。
    pub(crate) keymap: Keymap,

    /// 驻留卡片底边的关闭键提示(如 `x 关闭`),由 keymap 反查合成,随 keymap 重建刷新;
    /// 未绑定为空串(卡片不画 footer)。
    pub(crate) notice_hint: String,

    /// 浮层栈(queue / confirm / disconnect):统一托管开关、光标、弹出动画。
    pub(crate) overlays: OverlayStack,

    /// queue 浮层上次离开时的光标位置,下次打开落回原处;越界(队列已换短)时作废。
    pub(crate) queue_cursor_memo: Option<usize>,

    /// 整屏转场动画:`None` 为正常运行;`Some` 且 `leaving()` 为退出收缩(归零后真正退出),
    /// `Some` 且非 `leaving()` 为启动扩大(推满后转入正常运行)。两者时间上互斥,共用此字段。
    /// 退出仅正常退出(confirm)触发;Ctrl-C / 断连立即退,不走它。
    pub(crate) transition: Option<Transition>,

    /// Shift+Q「退出并停止 daemon」标记:置位后退出收缩动画收尾时向 daemon 投递
    /// shutdown(无视 `kill_spawned_daemon_on_exit` 旋钮;测试 client 是 no-op)。普通退出 / Ctrl-C / 断连不置位。
    pub(super) stop_daemon_on_quit: bool,

    /// 上一次 tick 时间。
    pub last_tick: Instant,

    /// 操作完成事件队列(后端写入,每帧 drain)。
    pub(super) completions: Arc<crate::runtime::backend::CompletionQueue>,

    /// Downloads 明细订阅是否处于打开状态(浮层生命周期驱动)。
    pub(super) downloads_subscribed: bool,

    /// Server client:所有「调命令 / 读镜像 / 取事件」都走它。
    /// 实现是 [`crate::runtime::backend::ClientBackend`](经 [`Backend`] 抽象);
    /// **player 业务在 daemon 端**;App 只 forward 意图并读订阅镜像。
    pub(crate) client: Arc<dyn Backend>,

    /// topbar 通知层:多条堆叠的提示通道(flash / 常驻进度),与具体业务解耦。
    pub(crate) notifications: Notifications,

    /// 下载 → 通知层的翻译器(持下载专属去重状态);通知层之上的众多使用方之一。
    pub(super) download_notifier: DownloadNotifier,

    /// 进 alternate screen 前捕获的终端光标位置,作为整屏 expand/collapse 的缩放锚点:
    /// expand 从此点铺开、collapse 收回此点(对得上 `LeaveAlternateScreen` 后光标实际
    /// 回到的行)。无 TTY 时为 `None`,缩放退化回屏幕居中。
    pub(crate) launch_anchor: Option<Position>,

    /// UI 偏好句柄:启动初值在 `App::new` 落地,运行时改动(`t` 键切歌词副轨档)
    /// fire-and-forget 落盘。
    pub(super) ui_prefs: UiPrefs,

    /// 上次上报 daemon 的终端状态 `(rows, cols, fullscreen, focused)`(去抖:值没变不发)。
    pub(super) last_terminal_report: Option<(u16, u16, bool, bool)>,

    /// 系统剪贴板句柄,首次复制时懒初始化并**终身持有**——X11 的剪贴板内容归
    /// owner 所有,句柄一 Drop 内容就没了;别改成每次复制临时 `new`。
    pub(crate) clipboard: Option<arboard::Clipboard>,

    /// 容器「播放全部 / 加入队列」的待兑现意图:key = [`crate::runtime::state::DetailFetch::dedup_key`],value = 入队
    /// 模式。容器曲目未加载时登记,对应 `*Fetched` 事件到货后由 [`Self::fulfill_pending_container`]
    /// 取载荷入队并清除(入队走 client,故意图落 App 而非 state)。同 key 后发覆盖、切走不命中即丢。
    pub(crate) pending_container: FxHashMap<String, PlayMode>,

    /// 首批可见时发起的起播意图；完整曲目到货后保留原始位置和过滤词兑现。
    pub(crate) pending_playlist_play: Option<crate::player_actions::PendingPlaylistPlay>,

    /// 终端窗口标题管理器（任务栏 / tab 标题）。
    pub(crate) window_title: WindowTitle,
}

impl App {
    /// 构造 App:主题 / 键表 / 各段手感由注入的配置一次性落地。
    ///
    /// # Params:
    ///   - `client`: 跟 server 交互的句柄
    ///   - `images`: 已完成 worker 与 terminal backend 接线的图片引擎
    ///   - `launch_anchor`: 进 alternate screen 前捕获的光标位置,作整屏 expand/collapse
    ///     的缩放锚点;`None`(无 TTY)时缩放退化回屏幕居中
    ///   - `cfg`: 已加载的全局配置(`Arc` 共享只读)
    ///   - `ui_prefs`: 已读回初值的 UI 偏好句柄(歌词副轨档在此落进 state)
    pub fn new(
        client: Arc<dyn Backend>,
        images: ImageEngine,
        launch_anchor: Option<Position>,
        cfg: Arc<mineral_config::Config>,
        ui_prefs: UiPrefs,
    ) -> Self {
        let completions = Arc::clone(client.completions());
        let tui_cfg = cfg.tui();
        let theme_base = Theme::from_config(tui_cfg.theme());
        let theme = Arc::new(theme_base);
        let mut keymap = Keymap::from_config(tui_cfg.keys(), tui_cfg.behavior());
        // 脚本 `mineral.bind` 的键合进查表(连接后一次拉齐的自举数据)。
        let bootstrap = client.bootstrap();
        keymap.append_script_binds(&bootstrap.script_binds);
        let anim = tui_cfg.animation();
        let tick_ms = *anim.frame_tick_ms();
        let accent_fade = crate::render::accent::AccentFade::new(
            crate::render::anim::ticks32_from_ms(*tui_cfg.theme().dynamic().fade_ms(), tick_ms),
        );
        let ambient = crate::render::ambient::AmbientGradient::new(
            crate::render::anim::ticks32_from_ms(*tui_cfg.ambient().fade_ms(), tick_ms),
            tick_ms,
        );
        let ambient_pulse = crate::render::ambient::LoudnessPulse::new(tick_ms);
        let overlays = OverlayStack::new(ticks16_from_ms(*anim.popup_anim_ms(), tick_ms));
        let notifications = Notifications::new(
            *tui_cfg.toast().flash_ttl_secs(),
            ticks16_from_ms(*anim.toast_anim_ms(), tick_ms),
        );
        let window_title = WindowTitle::new(tui_cfg.window_title());
        let mut state = AppState::new(cfg, images);
        // 各源能力声明:连接后自举一次进镜像,UI 据此画入口。
        state.caps = bootstrap.channel_caps.into_iter().collect();
        // 跨会话保留的歌词副轨档:即使当前歌缺该副轨,渲染端也会优雅回落原文。
        state.browse.lyric_view.extra = ui_prefs.initial_lyric_extra();
        // 跨会话保留的歌单位置记忆表:旋钮非 persist 档时灌了也只是闲置,
        // 不在这里判档——热重载切到 persist 后历史记忆立即可用。
        state.browse.nav.track_pos = ui_prefs.initial_track_pos().clone();
        let notice_hint = Self::compose_notice_hint(&keymap);
        Self {
            should_quit: false,
            theme,
            theme_base,
            accent_fade,
            ambient,
            ambient_pulse,
            state,
            keymap,
            notice_hint,
            overlays,
            queue_cursor_memo: None,
            transition: None,
            stop_daemon_on_quit: false,
            last_tick: Instant::now(),
            completions,
            downloads_subscribed: false,
            client,
            notifications,
            download_notifier: DownloadNotifier::new(),
            launch_anchor,
            ui_prefs,
            last_terminal_report: None,
            clipboard: None,
            pending_container: FxHashMap::default(),
            pending_playlist_play: None,
            window_title,
        }
    }
}

//! 持有当前 terminal graphics backend 的唯一运行时状态。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use arc_swap::ArcSwap;
use mineral_config::CoverProtocolMode;

use super::protocol::{GraphicsProtocol, TerminalRelay};
use super::query::DetectedGraphics;
use crate::image::kitty::probe_shared_memory;

/// 编码 worker 与图片引擎共享的当前 terminal backend。
///
/// 协议切换会原子替换完整状态并递增 generation；字号刷新只替换像素尺寸，generation
/// 保持不变。编码请求不复制协议状态，只用 generation 读取匹配的 backend。
#[derive(Clone)]
pub(crate) struct TerminalBackend {
    /// 当前 backend 状态的共享原子槽。
    current: Arc<ArcSwap<BackendState>>,
}

/// 一个 generation 对应的不可变 terminal backend 快照。
struct BackendState {
    /// terminal backend generation。
    generation: u64,

    /// 该 generation 唯一的终端图片能力。
    graphics: Arc<TerminalGraphics>,
}

impl TerminalBackend {
    /// 用启动探测结果与当前配置构造 terminal backend。
    ///
    /// # Params:
    ///   - `graphics`: 启动探测得到的终端图片能力
    ///   - `mode`: 当前配置的终端图片协议模式
    pub(crate) fn new(mut graphics: TerminalGraphics, mode: CoverProtocolMode) -> Self {
        let _ = graphics.apply_mode(mode);
        Self {
            current: Arc::new(ArcSwap::from_pointee(BackendState {
                generation: 0,
                graphics: Arc::new(graphics),
            })),
        }
    }

    /// 构造固定 cell 像素尺寸的 halfblocks backend。
    ///
    /// # Params:
    ///   - `cell_pixels`: 单个终端 cell 的像素宽高
    #[cfg(test)]
    pub(crate) fn fixed(cell_pixels: (u16, u16)) -> Self {
        Self::new(
            TerminalGraphics::fixed(cell_pixels),
            CoverProtocolMode::Halfblocks,
        )
    }

    /// 应用协议模式，生效协议变化时替换 backend 并返回 `true`。
    ///
    /// # Params:
    ///   - `mode`: 当前配置的终端图片协议模式
    pub(crate) fn apply_mode(&self, mode: CoverProtocolMode) -> bool {
        let current = self.current.load_full();
        let mut graphics = current.graphics.as_ref().clone();
        if !graphics.apply_mode(mode) {
            return false;
        }
        self.replace(Arc::new(graphics));
        true
    }

    /// 从当前终端窗口刷新 cell 像素尺寸，协议与 generation 保持不变。
    pub(crate) fn refresh_cell_pixels(&self) -> bool {
        let current = self.current.load_full();
        let mut graphics = current.graphics.as_ref().clone();
        if !graphics.refresh_cell_pixels() {
            return false;
        }
        self.current.store(Arc::new(BackendState {
            generation: current.generation,
            graphics: Arc::new(graphics),
        }));
        true
    }

    /// 返回当前生效协议。
    pub(crate) fn protocol(&self) -> GraphicsProtocol {
        self.current.load().graphics.protocol()
    }

    /// 返回当前单个终端 cell 的像素宽高。
    pub(crate) fn cell_pixels(&self) -> (u16, u16) {
        self.current.load().graphics.cell_pixels()
    }

    /// 返回当前 terminal backend generation。
    pub(crate) fn generation(&self) -> u64 {
        self.current.load().generation
    }

    /// 返回与请求 generation 匹配的终端图片能力。
    ///
    /// # Params:
    ///   - `generation`: 编码请求提交时的 backend generation
    pub(crate) fn graphics_for(&self, generation: u64) -> Option<Arc<TerminalGraphics>> {
        let current = self.current.load_full();
        (current.generation == generation).then(|| Arc::clone(&current.graphics))
    }

    /// 递增 generation 并原子替换终端图片能力。
    fn replace(&self, graphics: Arc<TerminalGraphics>) {
        let generation = self.current.load().generation.wrapping_add(1);
        self.current.store(Arc::new(BackendState {
            generation,
            graphics,
        }));
    }
}

/// 启动时协商出的终端图片能力及当前 cell 像素尺寸。
#[derive(Clone)]
pub(crate) struct TerminalGraphics {
    /// 当前生效的终端图片协议。
    protocol: GraphicsProtocol,

    /// 启动探测得到的协议，配置切回 auto 时恢复该值。
    negotiated: GraphicsProtocol,

    /// 单个终端 cell 的像素宽高。
    cell_pixels: (u16, u16),

    /// 自动模式命中的降级环境；强制模式忽略该信号。
    fallback_signal: Option<&'static str>,

    /// 当前终端是否能读取本进程创建的 POSIX shared memory。
    kitty_shared_memory: bool,

    /// 图形控制序列的终端 relay 形态。
    relay: TerminalRelay,

    /// 当前 terminal backend 共享的 Kitty image id 分配器。
    next_image_id: Arc<AtomicU32>,
}

impl TerminalGraphics {
    /// 从终端查询协议与 cell 像素尺寸。
    pub(crate) fn query() -> Self {
        let relay = TerminalRelay::from_environment();
        relay.prepare();
        let detected = super::query::detect(relay);
        let kitty_shared_memory = probe_shared_memory(relay);
        let negotiated = negotiated_protocol(&detected, relay, kitty_shared_memory);
        let cell_pixels = detected
            .cell_pixels
            .or_else(window_cell_pixels)
            .unwrap_or((10, 20));
        mineral_log::debug!(
            target: "tui",
            protocol = ?negotiated,
            cell_width = cell_pixels.0,
            cell_height = cell_pixels.1,
            kitty_shared_memory,
            sixel = detected.sixel,
            relay = ?relay,
            "终端图形能力已确定"
        );
        Self::from_parts(
            negotiated,
            cell_pixels,
            graphics_fallback_signal(),
            kitty_shared_memory,
            relay,
        )
    }

    /// 构造固定 cell 像素尺寸的 halfblocks 能力。
    ///
    /// # Params:
    ///   - `cell_pixels`: 单个终端 cell 的像素宽高
    #[cfg(test)]
    pub(crate) fn fixed(cell_pixels: (u16, u16)) -> Self {
        Self::from_parts(
            GraphicsProtocol::Halfblocks,
            cell_pixels,
            None,
            false,
            TerminalRelay::from_environment(),
        )
    }

    /// 返回当前生效的终端图片协议。
    pub(crate) const fn protocol(&self) -> GraphicsProtocol {
        self.protocol
    }

    /// 返回单个终端 cell 的像素宽高。
    pub(crate) const fn cell_pixels(&self) -> (u16, u16) {
        self.cell_pixels
    }

    /// 返回图形控制序列的终端 relay 形态。
    pub(crate) const fn relay(&self) -> TerminalRelay {
        self.relay
    }

    /// 分配一个非零且在当前 terminal backend 内不复用的 Kitty image id。
    pub(crate) fn allocate_kitty_image_id(&self) -> u32 {
        let image_id = self.next_image_id.fetch_add(1, Ordering::Relaxed);
        if image_id == 0 {
            self.next_image_id.fetch_add(1, Ordering::Relaxed)
        } else {
            image_id
        }
    }

    /// 应用图片协议配置，返回生效协议是否变化。
    ///
    /// # Params:
    ///   - `mode`: 当前配置的图片协议选择
    pub(crate) fn apply_mode(&mut self, mode: CoverProtocolMode) -> bool {
        let desired = resolved_protocol(
            mode,
            self.negotiated,
            self.fallback_signal,
            self.kitty_shared_memory,
        );
        if self.protocol == desired {
            return false;
        }
        if mode == CoverProtocolMode::Kitty && !self.kitty_shared_memory {
            mineral_log::warn!(
                target: "tui",
                fallback = ?desired,
                "Kitty shared memory 不可用，使用已协商的终端图协议"
            );
        } else if let Some(signal) = self.fallback_signal
            && desired == GraphicsProtocol::Halfblocks
        {
            mineral_log::warn!(
                target: "tui",
                signal,
                negotiated = ?self.negotiated,
                "图协议自动档降级半块字符;确认该环境可穿透渲染时可强制 tui.cover.protocol"
            );
        }
        self.protocol = desired;
        true
    }

    /// 从当前终端窗口尺寸刷新 cell 像素宽高，返回是否变化。
    ///
    /// 该查询不读取 stdin；终端未提供像素尺寸时保留现值。
    pub(crate) fn refresh_cell_pixels(&mut self) -> bool {
        window_cell_pixels().is_some_and(|cell_pixels| self.set_cell_pixels(cell_pixels))
    }

    /// 更新 cell 像素宽高，协议状态保持不变。
    fn set_cell_pixels(&mut self, cell_pixels: (u16, u16)) -> bool {
        if self.cell_pixels == cell_pixels {
            return false;
        }
        self.cell_pixels = cell_pixels;
        true
    }

    /// 从已经确定的能力构造 backend 状态。
    fn from_parts(
        negotiated: GraphicsProtocol,
        cell_pixels: (u16, u16),
        fallback_signal: Option<&'static str>,
        kitty_shared_memory: bool,
        relay: TerminalRelay,
    ) -> Self {
        let random_id = rand::random::<u32>();
        let first_image_id = if random_id == 0 { 1 } else { random_id };
        Self {
            protocol: negotiated,
            negotiated,
            cell_pixels,
            fallback_signal,
            kitty_shared_memory,
            relay,
            next_image_id: Arc::new(AtomicU32::new(first_image_id)),
        }
    }
}

/// 按环境提示与能力响应确定 auto 模式协议。
fn negotiated_protocol(
    detected: &DetectedGraphics,
    relay: TerminalRelay,
    kitty_shared_memory: bool,
) -> GraphicsProtocol {
    outer_protocol_hint(relay)
        .filter(|protocol| *protocol != GraphicsProtocol::Kitty || kitty_shared_memory)
        .or_else(iterm2_protocol_hint)
        .or_else(|| kitty_shared_memory.then_some(GraphicsProtocol::Kitty))
        .or_else(|| detected.sixel.then_some(GraphicsProtocol::Sixel))
        .unwrap_or(GraphicsProtocol::Halfblocks)
}

/// 返回 tmux 外层终端的协议提示。
fn outer_protocol_hint(relay: TerminalRelay) -> Option<GraphicsProtocol> {
    if !relay.is_tmux() {
        return None;
    }
    [
        ("KITTY_WINDOW_ID", GraphicsProtocol::Kitty),
        ("ITERM_SESSION_ID", GraphicsProtocol::Iterm2),
        ("WEZTERM_EXECUTABLE", GraphicsProtocol::Iterm2),
    ]
    .into_iter()
    .find_map(|(name, protocol)| {
        std::env::var(name)
            .is_ok_and(|value| !value.is_empty())
            .then_some(protocol)
    })
}

/// 返回声明支持 iTerm2 inline images 的终端环境提示。
fn iterm2_protocol_hint() -> Option<GraphicsProtocol> {
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    let supports_iterm2 = [
        "iTerm",
        "WezTerm",
        "mintty",
        "vscode",
        "Tabby",
        "Hyper",
        "rio",
        "Bobcat",
        "WarpTerminal",
    ]
    .into_iter()
    .any(|name| term_program.contains(name));
    (supports_iterm2
        || std::env::var("LC_TERMINAL").is_ok_and(|terminal| terminal.contains("iTerm")))
    .then_some(GraphicsProtocol::Iterm2)
}

/// 把配置选择解析为生效协议。
fn resolved_protocol(
    mode: CoverProtocolMode,
    negotiated: GraphicsProtocol,
    fallback_signal: Option<&'static str>,
    kitty_shared_memory: bool,
) -> GraphicsProtocol {
    match mode {
        CoverProtocolMode::Halfblocks => GraphicsProtocol::Halfblocks,
        CoverProtocolMode::Kitty if kitty_shared_memory => GraphicsProtocol::Kitty,
        CoverProtocolMode::Kitty => negotiated,
        CoverProtocolMode::Sixel => GraphicsProtocol::Sixel,
        CoverProtocolMode::Iterm2 => GraphicsProtocol::Iterm2,
        _ if fallback_signal.is_some() => GraphicsProtocol::Halfblocks,
        _ => negotiated,
    }
}

/// 返回自动协议模式命中的降级环境名称。
fn graphics_fallback_signal() -> Option<&'static str> {
    std::env::var("ZELLIJ")
        .is_ok_and(|value| !value.is_empty())
        .then_some("zellij")
}

/// 从终端窗口的像素与 cell 尺寸计算单 cell 像素宽高。
fn window_cell_pixels() -> Option<(u16, u16)> {
    let window = crossterm::terminal::window_size().ok()?;
    if window.columns == 0 || window.rows == 0 || window.width == 0 || window.height == 0 {
        return None;
    }
    Some((window.width / window.columns, window.height / window.rows))
}

#[cfg(test)]
mod tests {
    use mineral_config::CoverProtocolMode;

    use super::{GraphicsProtocol, TerminalGraphics, resolved_protocol};

    /// 终端字号变化保留协议并应用新像素尺寸。
    #[test]
    fn cell_pixel_update_preserves_protocol() {
        let mut graphics = TerminalGraphics::fixed((8, 16));
        let _ = graphics.apply_mode(CoverProtocolMode::Sixel);

        assert!(graphics.set_cell_pixels((10, 22)), "新 cell 像素尺寸应生效");
        assert_eq!(graphics.cell_pixels(), (10, 22));
        assert_eq!(graphics.protocol(), GraphicsProtocol::Sixel);
    }

    /// 自动模式服从协商与降级信号，强制模式直接选择指定协议。
    #[test]
    fn resolved_protocol_respects_mode_and_fallback() {
        assert_eq!(
            resolved_protocol(CoverProtocolMode::Auto, GraphicsProtocol::Kitty, None, true,),
            GraphicsProtocol::Kitty,
            "auto 无信号用协商结果",
        );
        assert_eq!(
            resolved_protocol(
                CoverProtocolMode::Auto,
                GraphicsProtocol::Kitty,
                Some("zellij"),
                true,
            ),
            GraphicsProtocol::Halfblocks,
            "auto 命中降级信号落半块",
        );
        assert_eq!(
            resolved_protocol(
                CoverProtocolMode::Kitty,
                GraphicsProtocol::Halfblocks,
                Some("zellij"),
                true,
            ),
            GraphicsProtocol::Kitty,
            "强制模式忽略降级信号",
        );
        assert_eq!(
            resolved_protocol(
                CoverProtocolMode::Kitty,
                GraphicsProtocol::Sixel,
                None,
                false,
            ),
            GraphicsProtocol::Sixel,
            "shared memory 不可用时强制 Kitty 仍使用协商结果",
        );
    }

    /// Kitty image id 在共享分配器中保持非零且唯一。
    #[test]
    fn kitty_image_ids_are_nonzero_and_unique_across_clones() {
        let graphics = TerminalGraphics::fixed((8, 16));
        let cloned = graphics.clone();
        let first = graphics.allocate_kitty_image_id();
        let second = cloned.allocate_kitty_image_id();
        assert_ne!(first, 0, "Kitty image id 不得为零");
        assert_ne!(second, 0, "Kitty image id 不得为零");
        assert_ne!(first, second, "worker clone 必须共享同一 id 序列");
    }
}

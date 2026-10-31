//! 本次 TUI 会话固定的图片协议、传输方式与 Kitty 图片身份分配。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Sender;

use crate::config::CoverProtocolMode;

use super::protocol::{GraphicsProtocol, TerminalRelay};
#[cfg(test)]
use crate::image::kitty::KittyImageRegistry;
use crate::image::kitty::{KittyImageEvent, probe_shared_memory};

/// 启动时选定的图片输出能力，由图片引擎与编码 worker 共享。
/// 协议和 relay 在会话内不变；显示尺寸由每次编码请求携带。
pub(crate) struct TerminalGraphics {
    /// 本次会话使用的图片协议。
    protocol: GraphicsProtocol,

    /// 控制序列是否经过 tmux passthrough。
    relay: TerminalRelay,

    /// 所有编码 worker 共用的 Kitty image id 分配器。
    next_image_id: AtomicU32,

    /// 图片上传与释放事件交给 TUI 会话提交。
    kitty_image_events: Sender<KittyImageEvent>,
}

impl TerminalGraphics {
    /// 启动时探测 Kitty 共享内存，按配置选择本次会话的协议。
    pub(crate) fn query(
        mode: CoverProtocolMode,
        kitty_image_events: Sender<KittyImageEvent>,
    ) -> Self {
        let relay = TerminalRelay::from_environment();
        relay.prepare();
        let kitty_shared_memory =
            mode != CoverProtocolMode::Halfblock && probe_shared_memory(relay);
        let zellij_pane = std::env::var("ZELLIJ").is_ok_and(|value| !value.is_empty());
        let protocol = resolved_protocol(mode, kitty_shared_memory, zellij_pane);
        match mode {
            CoverProtocolMode::Auto | CoverProtocolMode::Kitty if !kitty_shared_memory => {
                mineral_log::warn!(
                    target: "tui",
                    mode = ?mode,
                    "终端无法读取 Kitty 共享内存，Kitty 图协议不可用，使用半块字符"
                );
            }
            CoverProtocolMode::Auto if zellij_pane && kitty_shared_memory => {
                mineral_log::warn!(
                    target: "tui",
                    "zellij 内自动档使用半块字符；确认支持 Kitty 时可修改 cover.protocol 并重启"
                );
            }
            _ => {}
        }
        mineral_log::debug!(
            target: "tui",
            ?protocol,
            kitty_shared_memory,
            zellij_pane,
            ?relay,
            "本次会话的终端图片协议已确定"
        );
        Self::new(protocol, relay, kitty_image_events)
    }

    /// 构造不探测终端的半块输出能力。
    #[cfg(test)]
    pub(crate) fn fixed() -> Self {
        let (_, events) = KittyImageRegistry::new();
        Self::new(GraphicsProtocol::Halfblocks, TerminalRelay::Direct, events)
    }

    /// 构造不探测终端、允许 shared memory 编码的 Kitty 测试能力。
    #[cfg(test)]
    pub(crate) fn fixed_kitty() -> Self {
        let (_, events) = KittyImageRegistry::new();
        Self::new(GraphicsProtocol::Kitty, TerminalRelay::Direct, events)
    }

    pub(crate) const fn protocol(&self) -> GraphicsProtocol {
        self.protocol
    }

    pub(crate) const fn relay(&self) -> TerminalRelay {
        self.relay
    }

    pub(crate) fn kitty_image_events(&self) -> Sender<KittyImageEvent> {
        self.kitty_image_events.clone()
    }

    /// 为本次会话分配非零 Kitty image id，所有 worker 使用同一序列。
    pub(crate) fn allocate_kitty_image_id(&self) -> u32 {
        let image_id = self.next_image_id.fetch_add(1, Ordering::Relaxed);
        if image_id == 0 {
            self.next_image_id.fetch_add(1, Ordering::Relaxed)
        } else {
            image_id
        }
    }

    fn new(
        protocol: GraphicsProtocol,
        relay: TerminalRelay,
        kitty_image_events: Sender<KittyImageEvent>,
    ) -> Self {
        let random_id = rand::random::<u32>();
        let first_image_id = if random_id == 0 { 1 } else { random_id };
        Self {
            protocol,
            relay,
            next_image_id: AtomicU32::new(first_image_id),
            kitty_image_events,
        }
    }
}

/// Kitty 需要 shared memory 可读；auto 额外服从 zellij 降级信号。
fn resolved_protocol(
    mode: CoverProtocolMode,
    kitty_shared_memory: bool,
    zellij_pane: bool,
) -> GraphicsProtocol {
    let mode_allows_kitty = match mode {
        CoverProtocolMode::Halfblock => false,
        CoverProtocolMode::Kitty => true,
        CoverProtocolMode::Auto => !zellij_pane,
    };
    if mode_allows_kitty && kitty_shared_memory {
        GraphicsProtocol::Kitty
    } else {
        GraphicsProtocol::Halfblocks
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::config::CoverProtocolMode;

    use super::{GraphicsProtocol, TerminalGraphics, resolved_protocol};

    /// auto 需要 shared memory 且不在 zellij 内才用 Kitty，强制半块恒落半块。
    #[test]
    fn resolved_protocol_respects_mode_and_fallback() {
        assert_eq!(
            resolved_protocol(CoverProtocolMode::Auto, true, false),
            GraphicsProtocol::Kitty,
            "auto 在共享内存可用且非 zellij 时用 Kitty",
        );
        assert_eq!(
            resolved_protocol(CoverProtocolMode::Auto, true, true),
            GraphicsProtocol::Halfblocks,
            "auto 在 zellij 内降级半块",
        );
        assert_eq!(
            resolved_protocol(CoverProtocolMode::Auto, false, false),
            GraphicsProtocol::Halfblocks,
            "共享内存不可用时 auto 落半块",
        );
        assert_eq!(
            resolved_protocol(CoverProtocolMode::Kitty, true, true),
            GraphicsProtocol::Kitty,
            "强制 Kitty 忽略 zellij 降级",
        );
        assert_eq!(
            resolved_protocol(CoverProtocolMode::Kitty, false, false),
            GraphicsProtocol::Halfblocks,
            "共享内存不可用时强制 Kitty 落半块",
        );
        assert_eq!(
            resolved_protocol(CoverProtocolMode::Halfblock, true, false),
            GraphicsProtocol::Halfblocks,
            "强制半块不受终端能力影响",
        );
    }

    /// Kitty image id 在共享分配器中保持非零且唯一。
    #[test]
    fn kitty_image_ids_are_nonzero_and_unique_across_clones() {
        let graphics = Arc::new(TerminalGraphics::fixed());
        let cloned = Arc::clone(&graphics);
        let first = graphics.allocate_kitty_image_id();
        let second = cloned.allocate_kitty_image_id();
        assert_ne!(first, 0, "Kitty image id 不得为零");
        assert_ne!(second, 0, "Kitty image id 不得为零");
        assert_ne!(first, second, "worker clone 必须共享同一 id 序列");
    }
}

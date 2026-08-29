//! 终端图片协议与 relay 形态。

/// Mineral 支持的终端图片协议。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GraphicsProtocol {
    /// Kitty graphics protocol。
    Kitty,

    /// Sixel graphics protocol。
    Sixel,

    /// iTerm2 inline images protocol。
    Iterm2,

    /// 使用 cell 前景色与背景色绘制半块字符。
    Halfblocks,
}

/// 图形控制序列是否需要穿过 tmux passthrough。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalRelay {
    /// 直接写给终端。
    Direct,

    /// 写入 tmux DCS passthrough。
    Tmux,
}

impl TerminalRelay {
    /// 从终端环境变量识别 tmux relay。
    pub(crate) fn from_environment() -> Self {
        let term_is_tmux = std::env::var("TERM").is_ok_and(|term| term.starts_with("tmux"));
        let program_is_tmux = std::env::var("TERM_PROGRAM").is_ok_and(|program| program == "tmux");
        if term_is_tmux || program_is_tmux {
            Self::Tmux
        } else {
            Self::Direct
        }
    }

    /// 为当前 pane 开启 tmux passthrough。
    ///
    /// tmux 不可用或拒绝设置时保留 relay 形态，后续探测决定图形协议是否可用。
    pub(crate) fn prepare(self) {
        if self != Self::Tmux {
            return;
        }
        let status = std::process::Command::new("tmux")
            .args(["set", "-p", "allow-passthrough", "on"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        match status {
            Ok(status) if status.success() => {}
            Ok(status) => {
                mineral_log::warn!(
                    target: "tui",
                    code = status.code(),
                    "tmux allow-passthrough 设置失败"
                );
            }
            Err(error) => {
                let error = color_eyre::Report::new(error);
                mineral_log::warn!(
                    target: "tui",
                    error = mineral_log::chain(&error),
                    "tmux allow-passthrough 设置失败"
                );
            }
        }
    }

    /// 判断控制序列是否经过 tmux。
    pub(crate) fn is_tmux(self) -> bool {
        self == Self::Tmux
    }

    /// 按 relay 规则包装一组完整控制序列。
    pub(crate) fn wrap(self, sequence: String) -> String {
        if self == Self::Direct {
            return sequence;
        }
        let mut wrapped = String::with_capacity(sequence.len().saturating_add(16));
        wrapped.push_str("\x1bPtmux;");
        for character in sequence.chars() {
            if character == '\x1b' {
                wrapped.push('\x1b');
            }
            wrapped.push(character);
        }
        wrapped.push_str("\x1b\\");
        wrapped
    }
}

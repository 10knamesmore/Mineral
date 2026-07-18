//! 终端图片协议 picker 的运行期管理:配置强制项应用与字号刷新。
//!
//! 两条不变量贯穿本模块:① 协议类型只在启动 `from_query_stdio` 探测一次,之后的一切
//! 调整(强制项 / 字号重建)都**不重探**——重探往 stdio 写 escape 读响应,会跟事件循环
//! 抢 stdin 把按键吞掉;② 凡是换了协议类型或 cell 字号,必须清协议缓存逼全量重编码,
//! 否则旧编码按新参数 place 会错比例 / 悄悄降级。

use mineral_config::CoverProtocolMode;
use ratatui_image::picker::{Picker, ProtocolType};

impl crate::app::App {
    /// 现算并应用封面终端图协议(`cover.protocol` 强制项 / auto 档降级信号),
    /// 启动自举与配置热更共用;协议变了清协议缓存逼全量重编码。
    pub(crate) fn apply_cover_protocol(&mut self) {
        let mode = *self.state.cfg.tui().cover().protocol();
        let desired =
            resolved_cover_protocol(mode, self.negotiated_protocol, self.graphics_fallback);
        if self.picker.protocol_type() == desired {
            return;
        }
        if let Some(signal) = self.graphics_fallback
            && desired == ProtocolType::Halfblocks
        {
            mineral_log::warn!(
                target: "tui",
                signal,
                negotiated = ?self.negotiated_protocol,
                "图协议自动档降级半块字符;确认该环境可穿透渲染时可强制 tui.cover.protocol"
            );
        }
        self.picker.set_protocol_type(desired);
        self.state.covers.protocols.clear();
    }

    /// 终端字号 / 尺寸变化后刷新 `picker` 的 cell 像素尺寸(封面尺寸换算的基准)。
    ///
    /// 封面占多大由「cell 网格」决定,网格换算(`square_subarea` 与编码 `needs_resize`)都吃
    /// `picker.font_size()`。该值仅启动时 `from_query_stdio` 探一次;kitty 改字号时每 cell 的
    /// 像素尺寸变了却不刷新,封面就按旧 cell 比例铺 —— 字号变小占一小块、变大溢出被裁。
    ///
    /// 这里用 `window_size()`(TIOCGWINSZ syscall,不抢 stdin)重算 cell 像素,变了才重建
    /// picker + 清 `protocols` 逼下一帧按新字号重编码。
    /// window_size 的像素字段终端可能不实现(返 0),拿不到就保留旧值静默跳过。
    pub(crate) fn refresh_picker_font(&mut self) {
        let Ok(ws) = crossterm::terminal::window_size() else {
            return;
        };
        if ws.columns == 0 || ws.rows == 0 || ws.width == 0 || ws.height == 0 {
            return;
        }
        let font = (ws.width / ws.columns, ws.height / ws.rows);
        if font == self.picker.font_size() {
            return;
        }
        self.picker = rebuild_picker(&self.picker, font);
        // 清缓存协议:字号变但 cell 数恰好没变时 dims 不变、不会自动触发重编码,清掉逼重编。
        self.state.covers.protocols.clear();
    }
}

/// 把配置的协议选择落成 ratatui-image 协议类型:强制档直译(无视降级信号,是用户的
/// 明确逃生门);自动档有降级信号则半块字符,否则用探测协商结果。
///
/// # Params:
///   - `mode`: 配置 `cover.protocol`
///   - `negotiated`: 启动探测协商出的原始协议类型
///   - `fallback_signal`: 命中的降级信号([`graphics_fallback_signal`],`None` = 无)
///
/// # Return:
///   应生效的协议类型。
fn resolved_cover_protocol(
    mode: CoverProtocolMode,
    negotiated: ProtocolType,
    fallback_signal: Option<&'static str>,
) -> ProtocolType {
    match mode {
        CoverProtocolMode::Halfblocks => ProtocolType::Halfblocks,
        CoverProtocolMode::Kitty => ProtocolType::Kitty,
        CoverProtocolMode::Sixel => ProtocolType::Sixel,
        CoverProtocolMode::Iterm2 => ProtocolType::Iterm2,
        // 枚举在配置 crate 侧 non_exhaustive,wildcard 必需:auto 与未知新模式同路。
        _ => {
            if fallback_signal.is_some() {
                ProtocolType::Halfblocks
            } else {
                negotiated
            }
        }
    }
}

/// 探测图协议自动档的降级信号:「能力探测会穿透应答、渲染层却不合成图数据」的环境,
/// 协商出的协议在其中只会画成占位符乱码。每个信号一条独立判据,任一命中即降级;
/// 返回命中的信号名(进日志)。启动读一次(env 运行中不变)。
///
/// 判据靠 env 存在性,有假阳性(在该环境里新开的原生终端会继承 env)——
/// 这正是强制档无视信号的原因:误降的用户一行配置即可拿回图协议。
pub(crate) fn graphics_fallback_signal() -> Option<&'static str> {
    // zellij:kitty 能力查询穿透给宿主终端应答,合成层却不认 APC 图数据 / unicode
    // 占位符,占位符被当普通字形画出 → 成片乱码。(tmux 不在此列:ratatui-image
    // 自带 passthrough 支持,正常工作。)
    if std::env::var("ZELLIJ").is_ok_and(|v| !v.is_empty()) {
        return Some("zellij");
    }
    None
}

/// 按新 cell 字号重建 `picker`,**保留原协议类型**。
///
/// `Picker` 没有 font_size setter,只能重建;而 `from_fontsize` 会把协议从环境重新猜(kitty
/// 探测依赖 stdio query,重建时拿不到),默认落 halfblocks。故必须 `set_protocol_type` 把原
/// 协议塞回,否则 resize 后封面从 kitty/sixel 悄悄降级成半块字符。抽成自由函数以便单测该不变量。
fn rebuild_picker(old: &Picker, font: (u16, u16)) -> Picker {
    let mut picker = Picker::from_fontsize(font);
    picker.set_protocol_type(old.protocol_type());
    picker
}

#[cfg(test)]
mod tests {
    use mineral_config::CoverProtocolMode;
    use ratatui_image::picker::{Picker, ProtocolType};

    use super::{rebuild_picker, resolved_cover_protocol};

    /// 回归:终端字号变化后重建 picker 必须**保留原协议类型 + 应用新字号**。
    /// 若丢协议(`from_fontsize` 默认猜成 halfblocks),kitty/sixel 会悄悄降级成半块字符;
    /// 若不换字号,封面按旧 cell 比例铺 —— 偏小占一小块 / 偏大被裁。
    #[test]
    fn rebuild_picker_preserves_protocol_and_applies_font() {
        let mut old = Picker::from_fontsize((8, 16));
        old.set_protocol_type(ProtocolType::Kitty);

        let rebuilt = rebuild_picker(&old, (10, 22));

        assert_eq!(rebuilt.font_size(), (10, 22), "新 cell 字号应生效");
        assert_eq!(
            rebuilt.protocol_type(),
            ProtocolType::Kitty,
            "协议须保留为 kitty,不能被 from_fontsize 降级成 halfblocks",
        );
    }

    /// 协议落定表:auto 无信号尊重协商、有降级信号落半块;强制档无视协商与信号直译。
    #[test]
    fn resolved_protocol_respects_mode_and_fallback() {
        assert_eq!(
            resolved_cover_protocol(
                CoverProtocolMode::Auto,
                ProtocolType::Kitty,
                /*fallback_signal*/ None
            ),
            ProtocolType::Kitty,
            "auto 无信号用协商结果"
        );
        assert_eq!(
            resolved_cover_protocol(CoverProtocolMode::Auto, ProtocolType::Kitty, Some("zellij")),
            ProtocolType::Halfblocks,
            "auto 命中降级信号落半块(开箱不乱码)"
        );
        assert_eq!(
            resolved_cover_protocol(
                CoverProtocolMode::Halfblocks,
                ProtocolType::Kitty,
                /*fallback_signal*/ None
            ),
            ProtocolType::Halfblocks,
            "强制 halfblocks 无视协商"
        );
        assert_eq!(
            resolved_cover_protocol(
                CoverProtocolMode::Kitty,
                ProtocolType::Halfblocks,
                Some("zellij")
            ),
            ProtocolType::Kitty,
            "强制档无视降级信号(误降 / 确认穿透可用的逃生门)"
        );
    }
}

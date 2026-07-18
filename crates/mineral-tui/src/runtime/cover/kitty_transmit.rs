//! kitty 封面图数据的流式传输:把「首次显示才整段发终端」的 MB 级传输载荷,
//! 提前从已编码协议里取出、拆成完整转义单元,由主循环逐帧按字节预算写给终端。
//!
//! kitty graphics protocol(unicode placeholder 形态)分两段:**transmit**(APC `_G`
//! 命令链,把整图 base64 传给终端并建虚拟放置,本身不在屏上显示任何东西)与
//! **placement**(往 cell 写占位字符,几 KB)。ratatui-image 把 transmit 绑在首次
//! render:全屏封面数 MB 的序列挤进落定那一帧,终端一口气解析上传,可感卡顿。
//! 这里在编码结果装入协议缓存前先做一次 1×1 离屏 render 把 transmit 序列取出
//! (协议内部的一次性传输标志随之燃掉,之后真正 place 只写占位符),拆单元进
//! [`TransmitBacklog`],主循环每帧在两次 draw 之间直写终端若干 KB——传输摊平在
//! 转场 / 形变动画期间,落定帧零尖峰。
//!
//! 传完之前对应协议槽标记「待传输」不参与渲染命中(halfblock 兜底顶着),写完最后
//! 一个单元才放行 place——占位符绝不指向终端还没收全的图。
//!
//! 只对 kitty 生效:sixel / iTerm2 inline 的图数据即显示(escape 发出即画在光标处),
//! 没有「传而不显」的形态,提取直接返回 `None`,维持首显整段发送的原行为。

use std::collections::VecDeque;

use mineral_model::MediaUrl;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui_image::ResizeEncodeRender;
use ratatui_image::protocol::{StatefulProtocol, StatefulProtocolType};

/// placement 段起始标记(`CSI s`,保存光标位):kitty 渲染把「transmit 序列 +
/// 占位行」拼进同一 cell symbol,占位行固定以它开头,以此切分两段。
const PLACEMENT_MARKER: &str = "\x1b[s";

/// 转义单元终止符(`ESC \`,ST):transmit 序列是若干完整 APC 单元的拼接,
/// 只能在单元边界切块——写半个单元会把终端的转义解析状态搞脏。
const UNIT_TERMINATOR: &str = "\x1b\\";

/// tmux passthrough 前缀(DCS):tmux 下整段是单一 passthrough、内部转义被翻倍,
/// 不能按 [`UNIT_TERMINATOR`] 拆,整段当一个单元一次写出。
const TMUX_PREFIX: &str = "\x1bP";

/// 从刚编码好的协议里取出 kitty transmit 序列(若是 kitty 且尚未传输过)。
///
/// 通过一次 1×1 离屏 render 触发 ratatui-image 的一次性 transmit 拼接,再按
/// placement 起始标记把 transmit 前缀切出来。调用后协议内部的传输标志已燃:
/// 之后真正 place 只写占位符,**取出的序列必须由调用方送达终端**,否则图永远不显示。
///
/// # Params:
///   - `protocol`: 编码 worker 刚产出的协议(装入缓存前调用)
///
/// # Return:
///   transmit 序列;非 kitty 协议 / 已传输过返回 `None`。
pub(crate) fn extract_transmit(protocol: &mut StatefulProtocol) -> Option<String> {
    if !matches!(protocol.protocol_type(), StatefulProtocolType::Kitty(_)) {
        return None;
    }
    let probe = Rect::new(0, 0, 1, 1);
    let mut buf = Buffer::empty(probe);
    protocol.render(probe, &mut buf);
    let symbol = buf.cell((0, 0))?.symbol();
    let placement = symbol.find(PLACEMENT_MARKER)?;
    if placement == 0 {
        return None;
    }
    symbol.get(..placement).map(str::to_owned)
}

/// 一个在途的传输任务:一张封面在某编码尺寸下的完整 transmit 载荷,按转义单元拆块。
struct TransmitJob {
    /// 封面 URL(与协议缓存的槽对应)。
    url: MediaUrl,

    /// 编码尺寸键(与协议缓存的槽对应)。
    dims: (u16, u16),

    /// 尚未写出的转义单元,队首先写。
    units: VecDeque<String>,
}

/// 一次预算内弹出的批:待写终端的字节 + 恰好在本批传完的任务键。
///
/// 调用方必须**先把 `bytes` 写达终端、再应用 `completed`**(解除协议槽的待传输
/// 标记)——顺序反了占位符会先于图数据 place。
pub(crate) struct TransmitBatch {
    /// 本批要直写终端的转义序列(若干完整单元的拼接)。
    pub(crate) bytes: String,

    /// 本批写完后即完整送达的任务键(`(url, dims)`)。
    pub(crate) completed: Vec<(MediaUrl, (u16, u16))>,
}

/// kitty transmit backlog:FIFO 任务队列,每帧按字节预算弹出若干完整转义单元。
///
/// 任务严格顺序写出、互不交错——kitty 分块传输(`m=1` 链)按「当前在途传输」累积,
/// 两张图的 `_G` 单元交错会互相污染。占位符等非 `_G` 输出穿插无害。
#[derive(Default)]
pub(crate) struct TransmitBacklog {
    /// 排队中的传输任务,队首在写。
    jobs: VecDeque<TransmitJob>,
}

impl TransmitBacklog {
    /// 入队一张图的 transmit 载荷(内部按转义单元拆块;tmux passthrough 整段一个单元)。
    ///
    /// # Params:
    ///   - `url` / `dims`: 协议缓存的槽键,传完时随 [`TransmitBatch::completed`] 回吐
    ///   - `payload`: [`extract_transmit`] 取出的完整序列
    pub(crate) fn push(&mut self, url: MediaUrl, dims: (u16, u16), payload: String) {
        let units = split_units(payload);
        if units.is_empty() {
            return;
        }
        self.jobs.push_back(TransmitJob { url, dims, units });
    }

    /// 按字节预算弹出一批待写单元;空 backlog 返回 `None`。
    ///
    /// 预算是上限不是配额:单元只整个弹出,且**至少弹一个**保证前进——单个超预算
    /// 单元(tmux 整段)也会被写出,只是那一帧超支。
    ///
    /// # Params:
    ///   - `budget_bytes`: 本帧写出字节上限
    ///
    /// # Return:
    ///   待写批;无任务时 `None`。
    pub(crate) fn drain_budget(&mut self, budget_bytes: usize) -> Option<TransmitBatch> {
        let mut bytes = String::new();
        let mut completed = Vec::new();
        while let Some(job) = self.jobs.front_mut() {
            let Some(unit) = job.units.pop_front() else {
                self.jobs.pop_front();
                continue;
            };
            if !bytes.is_empty() && bytes.len().saturating_add(unit.len()) > budget_bytes {
                job.units.push_front(unit);
                break;
            }
            bytes.push_str(&unit);
            if job.units.is_empty() {
                completed.push((job.url.clone(), job.dims));
                self.jobs.pop_front();
            }
            if bytes.len() >= budget_bytes {
                break;
            }
        }
        (!bytes.is_empty()).then_some(TransmitBatch { bytes, completed })
    }
}

/// 把 transmit 载荷拆成完整转义单元(每单元自带终止符,可独立写出)。
///
/// tmux passthrough(DCS 前缀)整段是单一序列且内部转义翻倍,不可拆,整段一个单元。
///
/// # Params:
///   - `payload`: 完整 transmit 序列
///
/// # Return:
///   转义单元队列(顺序即写出顺序)。
fn split_units(payload: String) -> VecDeque<String> {
    if payload.starts_with(TMUX_PREFIX) {
        return VecDeque::from([payload]);
    }
    let mut units = VecDeque::new();
    let mut rest = payload.as_str();
    while let Some(pos) = rest.find(UNIT_TERMINATOR) {
        let end = pos.saturating_add(UNIT_TERMINATOR.len());
        if let Some(unit) = rest.get(..end) {
            units.push_back(unit.to_owned());
        }
        rest = rest.get(end..).unwrap_or_default();
    }
    if !rest.is_empty() {
        units.push_back(rest.to_owned());
    }
    units
}

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use image::{DynamicImage, RgbImage};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui_image::picker::{Picker, ProtocolType};
    use ratatui_image::protocol::StatefulProtocol;
    use ratatui_image::{Resize, ResizeEncodeRender};

    use super::{TransmitBacklog, extract_transmit, split_units};
    use mineral_model::MediaUrl;

    /// 造一个已按 `target` 编码好的 kitty 协议(强制协议类型,不依赖真实终端探测)。
    fn kitty_protocol(target: Rect) -> StatefulProtocol {
        let mut picker = Picker::from_fontsize((8, 16));
        picker.set_protocol_type(ProtocolType::Kitty);
        let image = DynamicImage::ImageRgb8(RgbImage::new(32, 32));
        let mut protocol = picker.new_resize_protocol(image);
        let resize = Resize::Scale(Some(image::imageops::FilterType::Triangle));
        if let Some(rect) = protocol.needs_resize(&resize, target) {
            protocol.resize_encode(&resize, rect);
        }
        protocol
    }

    /// 提取:kitty 协议首次取到完整 transmit 链(APC `_G` 开头、ST 结尾、带建图命令),
    /// 二次提取返回 `None`(一次性标志已燃)。
    #[test]
    fn extracts_kitty_transmit_once() -> color_eyre::Result<()> {
        let mut protocol = kitty_protocol(Rect::new(0, 0, 8, 4));
        let payload = extract_transmit(&mut protocol).ok_or_else(|| eyre!("首次提取应有载荷"))?;
        assert!(payload.starts_with("\x1b_G"), "载荷应以 APC _G 开头");
        assert!(payload.contains("a=T"), "载荷应含建图 + 虚拟放置命令");
        assert!(payload.ends_with("\x1b\\"), "载荷应以 ST 终止");
        assert_eq!(
            extract_transmit(&mut protocol),
            None,
            "传输标志已燃,二次提取应为 None"
        );
        Ok(())
    }

    /// 提取后真正渲染只写占位符:cell symbol 不再含 `_G` 数据链,placement 段完整。
    #[test]
    fn render_after_extract_is_placement_only() -> color_eyre::Result<()> {
        let target = Rect::new(0, 0, 8, 4);
        let mut protocol = kitty_protocol(target);
        let _ = extract_transmit(&mut protocol).ok_or_else(|| eyre!("首次提取应有载荷"))?;

        let mut buf = Buffer::empty(target);
        protocol.render(target, &mut buf);
        let symbol = buf
            .cell((0, 0))
            .ok_or_else(|| eyre!("cell (0,0) 越界"))?
            .symbol();
        assert!(!symbol.contains("_G"), "提取后渲染不应再带图数据");
        assert!(
            symbol.starts_with("\x1b[s"),
            "placement 段应完整(保存光标位开头)"
        );
        Ok(())
    }

    /// 非 kitty 协议(halfblocks)无从流式化,提取返回 `None`。
    #[test]
    fn non_kitty_extracts_none() {
        let image = DynamicImage::ImageRgb8(RgbImage::new(16, 16));
        let mut protocol = Picker::from_fontsize((8, 16)).new_resize_protocol(image);
        assert_eq!(extract_transmit(&mut protocol), None);
    }

    /// 拆单元:普通 kitty 链按 ST 边界拆,单元自带终止符,拼回原串。
    #[test]
    fn splits_units_at_st_boundaries() {
        let payload = "\x1b_Gm=1;AAAA\x1b\\\x1b_Gm=0;BBBB\x1b\\".to_owned();
        let units = split_units(payload.clone());
        assert_eq!(units.len(), 2, "两个 APC 单元");
        for unit in &units {
            assert!(unit.starts_with("\x1b_G"), "每单元应以 APC 开头");
            assert!(unit.ends_with("\x1b\\"), "每单元应以 ST 终止");
        }
        let joined = units.into_iter().collect::<String>();
        assert_eq!(joined, payload, "拆块不增不减字节");
    }

    /// tmux passthrough 整段一个单元(内部转义翻倍不可拆)。
    #[test]
    fn tmux_payload_is_single_unit() {
        let payload = "\x1bPtmux;\x1b\x1b_Gm=0;AA\x1b\x1b\\\x1b\\".to_owned();
        let units = split_units(payload.clone());
        assert_eq!(units.len(), 1, "tmux 整段不可拆");
        assert_eq!(units.front().cloned(), Some(payload));
    }

    /// backlog:预算内整单元弹出、至少前进一个单元、任务传完回吐完成键、FIFO 不交错。
    #[test]
    fn backlog_drains_by_budget_fifo() -> color_eyre::Result<()> {
        let (a, b) = (
            MediaUrl::remote("https://x.y/a.jpg")?,
            MediaUrl::remote("https://x.y/b.jpg")?,
        );
        let unit = |tag: &str| format!("\x1b_Gm=1;{tag}\x1b\\");
        let mut backlog = TransmitBacklog::default();
        backlog.push(
            a.clone(),
            (8, 4),
            format!("{}{}", unit("a1a1a1"), unit("a2a2a2")),
        );
        backlog.push(b.clone(), (8, 4), unit("b1b1b1"));

        // 预算 1 字节 < 单元长:仍弹出恰一个单元(保证前进),完成键为空。
        let first = backlog
            .drain_budget(/*budget_bytes*/ 1)
            .ok_or_else(|| eyre!("应有第一批"))?;
        assert_eq!(first.bytes, unit("a1a1a1"), "队首任务的首单元先写");
        assert!(first.completed.is_empty(), "任务 a 还有单元在途");

        // 大预算:a 的尾单元 + b 整个,一批带出两个完成键,顺序 FIFO。
        let second = backlog
            .drain_budget(/*budget_bytes*/ 1 << 20)
            .ok_or_else(|| eyre!("应有第二批"))?;
        assert_eq!(
            second.bytes,
            format!("{}{}", unit("a2a2a2"), unit("b1b1b1"))
        );
        assert_eq!(
            second.completed,
            vec![(a, (8, 4)), (b, (8, 4))],
            "两任务按 FIFO 依次传完"
        );

        assert!(
            backlog.drain_budget(/*budget_bytes*/ 1 << 20).is_none(),
            "空 backlog 应返回 None"
        );
        Ok(())
    }
}

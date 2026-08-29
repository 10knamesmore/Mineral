//! 把 Kitty unicode placeholder placement 写入 ratatui cell buffer。

use std::fmt::Write as _;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::command::create_virtual_placement;
use crate::image::graphics::TerminalRelay;

/// Kitty unicode placeholder codepoint。
const PLACEHOLDER: char = '\u{10EEEE}';

/// 协议 diacritic 表可表示的最大行列数。
pub(super) const MAX_PLACEMENT_CELLS: u16 = 297;

/// 把 cell 宽高限制在 unicode placeholder 可寻址范围内。
pub(super) fn clamp_cells(cells: (u16, u16)) -> (u16, u16) {
    (
        cells.0.min(MAX_PLACEMENT_CELLS),
        cells.1.min(MAX_PLACEMENT_CELLS),
    )
}

/// 把一个 virtual placement 写入 cell buffer。
///
/// 每行的完整 placeholder 串写进首个 cell，其余 cell 标记 skip，避免 ratatui diff
/// 在同一行插入额外输出。
///
/// # Params:
///   - `area`: 当前目标区域
///   - `buffer`: ratatui cell buffer
///   - `image_id`: Kitty image id
///   - `transmission`: 尚未发出的 shared memory transmit；仅拼进第一行一次
///   - `relay`: 终端 relay 形态
pub(super) fn render(
    area: Rect,
    buffer: &mut Buffer,
    image_id: u32,
    transmission: &mut Option<String>,
    relay: TerminalRelay,
) {
    let (width, height) = clamp_cells((area.width, area.height));
    if width == 0 || height == 0 {
        return;
    }
    let [id_high, id_red, id_green, id_blue] = image_id.to_be_bytes();
    let placement_id = placement_id(width, height);
    let [_, placement_red, placement_green, placement_blue] = placement_id.to_be_bytes();
    let id_color = format!("\x1b[38;2;{id_red};{id_green};{id_blue}m");
    let placement_color = format!("\x1b[58;2;{placement_red};{placement_green};{placement_blue}m");
    let mut pending = transmission.take().unwrap_or_default();
    pending.push_str(&create_virtual_placement(
        image_id,
        placement_id,
        (width, height),
        relay,
    ));
    for row in 0..height {
        let mut symbol = if row == 0 {
            std::mem::take(&mut pending)
        } else {
            String::new()
        };
        let width_usize = usize::from(width);
        symbol.reserve(
            id_color
                .len()
                .saturating_add(width_usize.saturating_mul(4))
                .saturating_add(32),
        );
        let _ = write!(
            symbol,
            "\x1b[s{id_color}{placement_color}{PLACEHOLDER}{}{}{}",
            diacritic(row),
            diacritic(0),
            diacritic(u16::from(id_high))
        );
        symbol.extend(std::iter::repeat_n(
            PLACEHOLDER,
            width_usize.saturating_sub(1),
        ));
        for column in 1..width {
            if let Some(cell) = buffer.cell_mut((area.left() + column, area.top() + row)) {
                cell.set_skip(true);
            }
        }
        let right = area.width.saturating_sub(1);
        let down = area.height.saturating_sub(1);
        let _ = write!(symbol, "\x1b[u\x1b[{right}C\x1b[{down}B");
        if let Some(cell) = buffer.cell_mut((area.left(), area.top() + row)) {
            cell.set_symbol(&symbol);
        }
    }
}

/// 把不超过 297 的 cell 宽高编码成非零 18-bit placement id。
fn placement_id(width: u16, height: u16) -> u32 {
    (u32::from(width) << 9) | u32::from(height)
}

/// 返回行、列或 image id 高字节对应的 diacritic。
fn diacritic(index: u16) -> char {
    ROW_COLUMN_DIACRITICS
        .get(usize::from(index))
        .copied()
        .unwrap_or('\u{0305}')
}

/// Kitty 官方定义的 row/column diacritics 表。
const ROW_COLUMN_DIACRITICS: [char; 297] = [
    '\u{0305}',
    '\u{030D}',
    '\u{030E}',
    '\u{0310}',
    '\u{0312}',
    '\u{033D}',
    '\u{033E}',
    '\u{033F}',
    '\u{0346}',
    '\u{034A}',
    '\u{034B}',
    '\u{034C}',
    '\u{0350}',
    '\u{0351}',
    '\u{0352}',
    '\u{0357}',
    '\u{035B}',
    '\u{0363}',
    '\u{0364}',
    '\u{0365}',
    '\u{0366}',
    '\u{0367}',
    '\u{0368}',
    '\u{0369}',
    '\u{036A}',
    '\u{036B}',
    '\u{036C}',
    '\u{036D}',
    '\u{036E}',
    '\u{036F}',
    '\u{0483}',
    '\u{0484}',
    '\u{0485}',
    '\u{0486}',
    '\u{0487}',
    '\u{0592}',
    '\u{0593}',
    '\u{0594}',
    '\u{0595}',
    '\u{0597}',
    '\u{0598}',
    '\u{0599}',
    '\u{059C}',
    '\u{059D}',
    '\u{059E}',
    '\u{059F}',
    '\u{05A0}',
    '\u{05A1}',
    '\u{05A8}',
    '\u{05A9}',
    '\u{05AB}',
    '\u{05AC}',
    '\u{05AF}',
    '\u{05C4}',
    '\u{0610}',
    '\u{0611}',
    '\u{0612}',
    '\u{0613}',
    '\u{0614}',
    '\u{0615}',
    '\u{0616}',
    '\u{0617}',
    '\u{0657}',
    '\u{0658}',
    '\u{0659}',
    '\u{065A}',
    '\u{065B}',
    '\u{065D}',
    '\u{065E}',
    '\u{06D6}',
    '\u{06D7}',
    '\u{06D8}',
    '\u{06D9}',
    '\u{06DA}',
    '\u{06DB}',
    '\u{06DC}',
    '\u{06DF}',
    '\u{06E0}',
    '\u{06E1}',
    '\u{06E2}',
    '\u{06E4}',
    '\u{06E7}',
    '\u{06E8}',
    '\u{06EB}',
    '\u{06EC}',
    '\u{0730}',
    '\u{0732}',
    '\u{0733}',
    '\u{0735}',
    '\u{0736}',
    '\u{073A}',
    '\u{073D}',
    '\u{073F}',
    '\u{0740}',
    '\u{0741}',
    '\u{0743}',
    '\u{0745}',
    '\u{0747}',
    '\u{0749}',
    '\u{074A}',
    '\u{07EB}',
    '\u{07EC}',
    '\u{07ED}',
    '\u{07EE}',
    '\u{07EF}',
    '\u{07F0}',
    '\u{07F1}',
    '\u{07F3}',
    '\u{0816}',
    '\u{0817}',
    '\u{0818}',
    '\u{0819}',
    '\u{081B}',
    '\u{081C}',
    '\u{081D}',
    '\u{081E}',
    '\u{081F}',
    '\u{0820}',
    '\u{0821}',
    '\u{0822}',
    '\u{0823}',
    '\u{0825}',
    '\u{0826}',
    '\u{0827}',
    '\u{0829}',
    '\u{082A}',
    '\u{082B}',
    '\u{082C}',
    '\u{082D}',
    '\u{0951}',
    '\u{0953}',
    '\u{0954}',
    '\u{0F82}',
    '\u{0F83}',
    '\u{0F86}',
    '\u{0F87}',
    '\u{135D}',
    '\u{135E}',
    '\u{135F}',
    '\u{17DD}',
    '\u{193A}',
    '\u{1A17}',
    '\u{1A75}',
    '\u{1A76}',
    '\u{1A77}',
    '\u{1A78}',
    '\u{1A79}',
    '\u{1A7A}',
    '\u{1A7B}',
    '\u{1A7C}',
    '\u{1B6B}',
    '\u{1B6D}',
    '\u{1B6E}',
    '\u{1B6F}',
    '\u{1B70}',
    '\u{1B71}',
    '\u{1B72}',
    '\u{1B73}',
    '\u{1CD0}',
    '\u{1CD1}',
    '\u{1CD2}',
    '\u{1CDA}',
    '\u{1CDB}',
    '\u{1CE0}',
    '\u{1DC0}',
    '\u{1DC1}',
    '\u{1DC3}',
    '\u{1DC4}',
    '\u{1DC5}',
    '\u{1DC6}',
    '\u{1DC7}',
    '\u{1DC8}',
    '\u{1DC9}',
    '\u{1DCB}',
    '\u{1DCC}',
    '\u{1DD1}',
    '\u{1DD2}',
    '\u{1DD3}',
    '\u{1DD4}',
    '\u{1DD5}',
    '\u{1DD6}',
    '\u{1DD7}',
    '\u{1DD8}',
    '\u{1DD9}',
    '\u{1DDA}',
    '\u{1DDB}',
    '\u{1DDC}',
    '\u{1DDD}',
    '\u{1DDE}',
    '\u{1DDF}',
    '\u{1DE0}',
    '\u{1DE1}',
    '\u{1DE2}',
    '\u{1DE3}',
    '\u{1DE4}',
    '\u{1DE5}',
    '\u{1DE6}',
    '\u{1DFE}',
    '\u{20D0}',
    '\u{20D1}',
    '\u{20D4}',
    '\u{20D5}',
    '\u{20D6}',
    '\u{20D7}',
    '\u{20DB}',
    '\u{20DC}',
    '\u{20E1}',
    '\u{20E7}',
    '\u{20E9}',
    '\u{20F0}',
    '\u{2CEF}',
    '\u{2CF0}',
    '\u{2CF1}',
    '\u{2DE0}',
    '\u{2DE1}',
    '\u{2DE2}',
    '\u{2DE3}',
    '\u{2DE4}',
    '\u{2DE5}',
    '\u{2DE6}',
    '\u{2DE7}',
    '\u{2DE8}',
    '\u{2DE9}',
    '\u{2DEA}',
    '\u{2DEB}',
    '\u{2DEC}',
    '\u{2DED}',
    '\u{2DEE}',
    '\u{2DEF}',
    '\u{2DF0}',
    '\u{2DF1}',
    '\u{2DF2}',
    '\u{2DF3}',
    '\u{2DF4}',
    '\u{2DF5}',
    '\u{2DF6}',
    '\u{2DF7}',
    '\u{2DF8}',
    '\u{2DF9}',
    '\u{2DFA}',
    '\u{2DFB}',
    '\u{2DFC}',
    '\u{2DFD}',
    '\u{2DFE}',
    '\u{2DFF}',
    '\u{A66F}',
    '\u{A67C}',
    '\u{A67D}',
    '\u{A6F0}',
    '\u{A6F1}',
    '\u{A8E0}',
    '\u{A8E1}',
    '\u{A8E2}',
    '\u{A8E3}',
    '\u{A8E4}',
    '\u{A8E5}',
    '\u{A8E6}',
    '\u{A8E7}',
    '\u{A8E8}',
    '\u{A8E9}',
    '\u{A8EA}',
    '\u{A8EB}',
    '\u{A8EC}',
    '\u{A8ED}',
    '\u{A8EE}',
    '\u{A8EF}',
    '\u{A8F0}',
    '\u{A8F1}',
    '\u{AAB0}',
    '\u{AAB2}',
    '\u{AAB3}',
    '\u{AAB7}',
    '\u{AAB8}',
    '\u{AABE}',
    '\u{AABF}',
    '\u{AAC1}',
    '\u{FE20}',
    '\u{FE21}',
    '\u{FE22}',
    '\u{FE23}',
    '\u{FE24}',
    '\u{FE25}',
    '\u{FE26}',
    '\u{10A0F}',
    '\u{10A38}',
    '\u{1D185}',
    '\u{1D186}',
    '\u{1D187}',
    '\u{1D188}',
    '\u{1D189}',
    '\u{1D1AA}',
    '\u{1D1AB}',
    '\u{1D1AC}',
    '\u{1D1AD}',
    '\u{1D242}',
    '\u{1D243}',
    '\u{1D244}',
];

#[cfg(test)]
mod tests {
    use color_eyre::eyre::eyre;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;

    use super::render;
    use crate::image::graphics::TerminalRelay;

    /// placement 首 cell 编码 image id、行列，同行其余 cell 标记 skip。
    #[test]
    fn placement_encodes_id_rows_and_columns() -> color_eyre::Result<()> {
        let area = Rect::new(0, 0, 3, 2);
        let mut buffer = Buffer::empty(area);
        let mut transmission = None;
        render(
            area,
            &mut buffer,
            /*image_id*/ 0x0A0B_0C0D,
            &mut transmission,
            TerminalRelay::Direct,
        );

        let first = buffer
            .cell((0, 0))
            .ok_or_else(|| eyre!("缺少首个 placement cell"))?;
        assert!(
            first
                .symbol()
                .contains("i=168496141,p=1538,a=p,U=1,c=3,r=2"),
            "首行应创建当前尺寸的 virtual placement"
        );
        assert!(
            first
                .symbol()
                .contains("\x1b[38;2;11;12;13m\x1b[58;2;0;6;2m"),
            "placeholder 应编码 image id 与 placement id"
        );
        assert!(
            buffer.cell((1, 0)).is_some_and(|cell| cell.skip),
            "同一 placement 行的后续 cell 须跳过 diff 输出"
        );
        let second = buffer
            .cell((0, 1))
            .ok_or_else(|| eyre!("缺少第二行 placement cell"))?;
        assert!(
            second
                .symbol()
                .contains("\u{10EEEE}\u{030D}\u{0305}\u{034B}"),
            "第二行须使用下一项 row diacritic"
        );
        Ok(())
    }

    /// 尚未写出的 transmit 只拼入第一行一次。
    #[test]
    fn placement_consumes_transmission_once() -> color_eyre::Result<()> {
        let area = Rect::new(0, 0, 1, 2);
        let mut buffer = Buffer::empty(area);
        let mut transmission = Some("firstsecond".to_owned());
        render(
            area,
            &mut buffer,
            /*image_id*/ 1,
            &mut transmission,
            TerminalRelay::Direct,
        );
        assert!(transmission.is_none(), "render 后 transmit 应被消费");
        assert!(
            buffer
                .cell((0, 0))
                .is_some_and(|cell| cell.symbol().starts_with("firstsecond")),
            "首行应携带完整 transmit"
        );
        assert!(
            buffer
                .cell((0, 1))
                .is_some_and(|cell| !cell.symbol().contains("first")),
            "后续行不应重复 transmit"
        );
        Ok(())
    }
}

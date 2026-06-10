//! 多行通知卡片:圆角边框 + 标题 + 多行 body(行内 spans 样式)+ 底边关闭键提示。
//!
//! 与单行 [`crate::components::toast::toast::Toast`] 平行的渲染单元:内容是协议的
//! 结构化 spans([`TextSpan`],fg 主题角色在渲染时经 [`Theme`] 落色),不走
//! `ToastItem` trait —— 卡片没有"自定义绘制"的开放需求,直接持有数据自渲染。
//! 生命周期:带 deadline 的到时自动退场,否则驻留到显式 [`Card::dismiss`]。
//! 进出场方向由渲染时的 [`CardMotion`] 决定(不存进卡片状态),布局模式切换零迁移。

use std::time::{Duration, Instant};

use mineral_protocol::{SpanAlign, SpanFg, TextSpan};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding};
use unicode_width::UnicodeWidthStr;

use crate::components::toast::notifications::TextTint;
use crate::render::anim::Transition;
use crate::render::theme::Theme;

/// 把一段纯文本升成无样式单 span 行(标题等单行语境的便捷构造)。
///
/// # Params:
///   - `text`: 纯文本(空串得空行 = 不画)
///
/// # Return:
///   单 span、全默认样式的行;空串为空 vec。
pub(crate) fn plain_line(text: impl Into<String>) -> Vec<TextSpan> {
    let text = text.into();
    if text.is_empty() {
        Vec::new()
    } else {
        vec![TextSpan::plain(text)]
    }
}

/// 把纯文本行升成无样式 spans body(TUI 内部调用方 / 测试的便捷构造)。
///
/// # Params:
///   - `lines`: 纯文本行
///
/// # Return:
///   每行单 span、全默认样式的 body。
pub(crate) fn plain_body(lines: impl IntoIterator<Item = String>) -> Vec<Vec<TextSpan>> {
    lines
        .into_iter()
        .map(|l| vec![TextSpan::plain(l)])
        .collect::<Vec<Vec<TextSpan>>>()
}

/// 卡片进出场动画的方向(由调用方按当前布局传入)。
#[derive(Clone, Copy)]
pub(crate) enum CardMotion {
    /// 顶边锚定、高度随动画向下展开 / 收起(Full / Compact 顶部居中用)。
    ExpandDown,

    /// 右缘锚定、宽度随动画自右向左滑入 / 滑出(immersive 右上角用)。
    SlideInRight,
}

/// 带 TTL 卡片的存活窗口:到期时刻 + 总时长(边框倒计时蔓延的进度基准)。
#[derive(Clone, Copy)]
pub(crate) struct CardTtl {
    /// 到期时刻(过点自动退场)。
    pub(crate) deadline: Instant,

    /// 总时长(蔓延进度 = 已消耗 / 总时长)。
    pub(crate) total: Duration,
}

/// 一张通知卡片:级别 + 标题 + 多行 spans body + 进出场动画 + 可选到期时刻。
pub(crate) struct Card {
    /// 语义级别(决定边框 / 标题色与标题符号)。
    tint: TextTint,

    /// 标题(行内 spans,画进上边框;不含级别符号,符号渲染时按 `tint` 补)。
    title: Vec<TextSpan>,

    /// body:外层 = 行,内层 = 行内 spans(超出卡片宽度的行渲染时截断)。
    body: Vec<Vec<TextSpan>>,

    /// 顶替键:同 id 的后续卡片替换本张内容(复活退场中的);`None` 不参与顶替。
    id: Option<String>,

    /// 进出场动画(进场 = 出现,退场 = dismiss 后收起)。
    anim: Transition,

    /// 存活窗口:`Some` 过点自动退场(管理器 prune 触发)、边框暗色随消耗
    /// 自左上向右下蔓延;`None` 驻留到显式关闭,边框常亮。
    ttl: Option<CardTtl>,
}

impl Card {
    /// 新建并立即进场。
    ///
    /// # Params:
    ///   - `tint`: 语义级别
    ///   - `title`: 标题 spans(画进边框;空 = 不画)
    ///   - `body`: 行 / spans
    ///   - `id`: 顶替键;`None` 不参与顶替
    ///   - `anim_ticks`: 进 / 出场动画 tick 数(配置 `animation.toast_anim_ticks`)
    ///   - `ttl`: 存活窗口;`None` 驻留
    pub(crate) fn new(
        tint: TextTint,
        title: Vec<TextSpan>,
        body: Vec<Vec<TextSpan>>,
        id: Option<String>,
        anim_ticks: u16,
        ttl: Option<CardTtl>,
    ) -> Self {
        let mut anim = Transition::new(anim_ticks);
        anim.enter();
        Self {
            tint,
            title,
            body,
            id,
            anim,
            ttl,
        }
    }

    /// 顶替键(管理器做同 id 替换用)。
    pub(crate) fn id(&self) -> Option<&str> {
        self.id.as_deref()
    }

    /// 是否已过期(到点该自动退场;驻留卡恒否)。
    ///
    /// # Params:
    ///   - `now`: 当前时刻(测试可注入)
    pub(crate) fn expired(&self, now: Instant) -> bool {
        self.ttl.is_some_and(|t| now >= t.deadline)
    }

    /// 替换内容并(重新)进场 —— 同 id 顶替语义,退场中也复活;存活窗口一并刷新。
    ///
    /// # Params:
    ///   - `tint`: 新语义级别
    ///   - `title`: 新标题 spans
    ///   - `body`: 新行 / spans
    ///   - `ttl`: 新存活窗口;`None` 转驻留
    pub(crate) fn refresh(
        &mut self,
        tint: TextTint,
        title: Vec<TextSpan>,
        body: Vec<Vec<TextSpan>>,
        ttl: Option<CardTtl>,
    ) {
        self.tint = tint;
        self.title = title;
        self.body = body;
        self.ttl = ttl;
        self.anim.enter();
    }

    /// 触发退场(动画收完后 [`Self::dormant`] 为真,由管理器移除)。
    pub(crate) fn dismiss(&mut self) {
        self.anim.leave();
    }

    /// 是否在退场中(管理器跳过已 dismiss 的卡片,避免重复关)。
    pub(crate) fn leaving(&self) -> bool {
        self.anim.leaving()
    }

    /// 推进进出场动画一拍。
    pub(crate) fn tick(&mut self) {
        self.anim.tick();
    }

    /// 是否彻底休眠(退场归零)—— 管理器可安全移除。
    pub(crate) fn dormant(&self) -> bool {
        !self.anim.active()
    }

    /// 完全展开时的总宽(含边框与左右 padding),按标题 / body / 关闭提示取最宽。
    ///
    /// # Params:
    ///   - `close_hint`: 底边关闭键提示文本(参与量宽)
    pub(crate) fn width(&self, close_hint: &str) -> u16 {
        let title = line_width(&self.title).saturating_add(symbol_width(self.tint));
        let body = self.body.iter().map(|l| line_width(l)).max().unwrap_or(0);
        let hint = text_width(close_hint);
        // 边框 2 + 左右 padding 2;标题 / 底边提示自带前后各一空格(+2)。
        title
            .max(hint)
            .saturating_add(2)
            .max(body)
            .saturating_add(4)
    }

    /// 完全展开时的总高(body 行数 + 上下边框)。
    pub(crate) fn height(&self) -> u16 {
        u16::try_from(self.body.len())
            .unwrap_or(u16::MAX)
            .saturating_add(2)
    }

    /// 在 `slot`(完全展开尺寸的目标位)渲染:进出场途中沿 `motion` 方向画
    /// 1/8 cell 精度的纯色空壳(无边框 / 无内容,消除整格台阶与 reflow 抖动,
    /// 与浮层弹出动画同范式),到位后切真卡片。
    ///
    /// # Params:
    ///   - `frame`: 目标帧
    ///   - `slot`: 完全展开时的卡片矩形(调用方已 clamp 进屏幕)
    ///   - `motion`: 进出场方向
    ///   - `close_hint`: 底边关闭键提示(如 `[X] 关闭`)
    ///   - `theme`: 配色
    ///   - `now`: 当前时刻(带 TTL 卡的边框倒计时蔓延按它取进度;测试可注入)
    pub(crate) fn render(
        &self,
        frame: &mut Frame<'_>,
        slot: Rect,
        motion: CardMotion,
        close_hint: &str,
        theme: &Theme,
        now: Instant,
    ) {
        if !self.anim.active() || slot.width < 2 || slot.height < 2 {
            return;
        }
        let eased = self.anim.eased_in_out();
        if eased < 1000 {
            match motion {
                CardMotion::ExpandDown => draw_v_grow_shell(frame, slot, eased, theme),
                CardMotion::SlideInRight => draw_h_grow_shell(frame, slot, eased, theme),
            }
            return;
        }
        let area = slot;
        let accent = self.accent_color(theme);
        let mut block = Block::new()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(accent))
            .style(Style::new().bg(theme.base))
            .padding(Padding::horizontal(1));
        if !self.title.is_empty() || !symbol(self.tint).is_empty() {
            // 标题行:` 符号 + 标题 spans `,缺省色 = 级别色,span 自带样式可覆盖。
            let mut heading = vec![Span::styled(
                format!(" {}", symbol(self.tint)),
                Style::new().fg(accent),
            )];
            heading.extend(
                self.title
                    .iter()
                    .map(|s| Span::styled(s.text.as_str(), span_style(s, accent, theme))),
            );
            heading.push(Span::raw(" "));
            block = block.title(Line::from(heading));
        }
        if !close_hint.is_empty() {
            block = block.title_bottom(
                Line::from(format!(" {close_hint} "))
                    .style(Style::new().fg(theme.overlay))
                    .alignment(Alignment::Right),
            );
        }
        let inner = block.inner(area);
        frame.render_widget(Clear, area);
        frame.render_widget(block, area);
        // 部分展开时只画放得下的行;行内按 span 段位三组布局,超宽截断。
        for (i, spans) in self.body.iter().take(usize::from(inner.height)).enumerate() {
            let row = Rect::new(
                inner.x,
                inner.y.saturating_add(u16::try_from(i).unwrap_or(u16::MAX)),
                inner.width,
                1,
            );
            render_line(frame, row, spans, theme);
        }
        let progress = self.burn_progress(now);
        if progress > 0 {
            decay_border(frame.buffer_mut(), area, progress, accent, theme);
        }
    }

    /// 级别 → 边框 / 标题色。
    fn accent_color(&self, theme: &Theme) -> Color {
        match self.tint {
            TextTint::Normal => theme.accent,
            TextTint::Warn => theme.yellow,
            TextTint::Error => theme.red,
        }
    }

    /// TTL 已消耗比例(千分比):0 = 刚出现 / 驻留卡,1000 = 到期。
    /// 边框倒计时蔓延按它推进。
    ///
    /// # Params:
    ///   - `now`: 当前时刻
    fn burn_progress(&self, now: Instant) -> u16 {
        let Some(ttl) = self.ttl else {
            return 0;
        };
        let total = ttl.total.as_millis();
        if total == 0 {
            return 1000;
        }
        let remaining = ttl.deadline.saturating_duration_since(now).as_millis();
        let consumed = total.saturating_sub(remaining);
        u16::try_from(consumed.saturating_mul(1000) / total).unwrap_or(1000)
    }
}

/// 同段(连排)spans 之间的最小间隙(字符),量宽时按非空段数累加。
const GROUP_GAP: u16 = 2;

/// 在一行内按 span 段位三组布局渲染:左组贴左、中组居中、右组贴右,
/// 段内按原顺序连排。每组拼成一个 [`Line`](对齐交给 ratatui),叠画到
/// 同一 row;后画的段覆盖先画的(卡片量宽已保证默认不重叠,真重叠
/// 只发生在调用方塞超宽内容时)。
///
/// # Params:
///   - `frame`: 目标帧
///   - `row`: 该行的内区(高 1)
///   - `spans`: 行内 spans
///   - `theme`: 配色(fg 角色落色)
fn render_line(frame: &mut Frame<'_>, row: Rect, spans: &[TextSpan], theme: &Theme) {
    if row.width == 0 || row.height == 0 {
        return;
    }
    for (align, alignment) in [
        (SpanAlign::Left, Alignment::Left),
        (SpanAlign::Center, Alignment::Center),
        (SpanAlign::Right, Alignment::Right),
    ] {
        let group = spans
            .iter()
            .filter(|s| s.align == align)
            .map(|s| Span::styled(s.text.as_str(), span_style(s, theme.text, theme)))
            .collect::<Vec<Span<'_>>>();
        if group.is_empty() {
            continue;
        }
        frame.render_widget(Line::from(group).alignment(alignment), row);
    }
}

/// 单 span 的样式:fg 角色经主题落色(缺省给 `default_fg`,语境决定 ——
/// 卡片 body 是正文色、卡片标题是级别色、单行 flash 是 tint 色),
/// 修饰位逐项映射。背景不设,落在调用方已铺好的底色上。
pub(crate) fn span_style(span: &TextSpan, default_fg: Color, theme: &Theme) -> Style {
    let fg = match span.fg {
        None => default_fg,
        Some(SpanFg::Text) => theme.text,
        Some(SpanFg::Subtext) => theme.subtext,
        Some(SpanFg::Overlay) => theme.overlay,
        Some(SpanFg::Accent) => theme.accent,
        Some(SpanFg::Red) => theme.red,
        Some(SpanFg::Yellow) => theme.yellow,
        Some(SpanFg::Green) => theme.green,
        Some(SpanFg::Peach) => theme.peach,
        Some(SpanFg::Rgb(r, g, b)) => Color::Rgb(r, g, b),
    };
    let mut style = Style::new().fg(fg);
    if span.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.underline {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if span.dim {
        style = style.add_modifier(Modifier::DIM);
    }
    style
}

/// 一行 spans 按段位分组后的各组宽。
struct GroupWidths {
    /// 左段总宽。
    left: u16,

    /// 中段总宽。
    center: u16,

    /// 右段总宽。
    right: u16,
}

/// 各段位组的显示宽(组内 span 文本宽之和,u16 饱和)。
fn group_widths(spans: &[TextSpan]) -> GroupWidths {
    let mut w = GroupWidths {
        left: 0,
        center: 0,
        right: 0,
    };
    for span in spans {
        let dst = match span.align {
            SpanAlign::Left => &mut w.left,
            SpanAlign::Center => &mut w.center,
            SpanAlign::Right => &mut w.right,
        };
        *dst = dst.saturating_add(text_width(&span.text));
    }
    w
}

/// 一行 spans 的最小所需宽:各段宽之和 + 非空段间隙([`GROUP_GAP`] × 段界数)。
/// 卡片按它量宽,保证三段布局默认不重叠。
fn line_width(spans: &[TextSpan]) -> u16 {
    let w = group_widths(spans);
    let groups = u16::from(w.left > 0) + u16::from(w.center > 0) + u16::from(w.right > 0);
    [w.left, w.center, w.right]
        .into_iter()
        .fold(0_u16, u16::saturating_add)
        .saturating_add(GROUP_GAP.saturating_mul(groups.saturating_sub(1)))
}

/// ExpandDown 的进退场空壳:顶边锚定,高度按 `scale`(千分比)向下长出 / 收回,
/// 生长的底缘用 1/8 cell 八分块平滑(反色补齐:cell 上部实心、下部露背景)。
/// 体色随 scale 从骨架色 surface1 渐变到卡片体色 base,到位切真卡片时体色连续。
fn draw_v_grow_shell(frame: &mut Frame<'_>, full: Rect, scale: u16, theme: &Theme) {
    if full.width == 0 || full.height == 0 {
        return;
    }
    let full_h_e = u32::from(full.height) * 8;
    let cur_h_e = full_h_e * u32::from(scale) / 1000;
    // 不足一格画不出有意义的面板,跳过这一帧。
    if cur_h_e < 8 {
        return;
    }
    let top_e = u32::from(full.y) * 8;
    let bottom_e = top_e + cur_h_e;
    let row1 = bottom_e.div_ceil(8);
    let rows = u16::try_from(row1.saturating_sub(u32::from(full.y))).unwrap_or(full.height);
    frame.render_widget(Clear, Rect::new(full.x, full.y, full.width, rows));

    let fill = shell_fill(scale, theme);
    let bg = theme.base;
    let buf = frame.buffer_mut();
    for row in u32::from(full.y)..row1 {
        let r_lo = row * 8;
        let vcov = bottom_e.min(r_lo + 8).saturating_sub(top_e.max(r_lo));
        if vcov == 0 {
            continue;
        }
        let (glyph, style) = if vcov >= 8 {
            ("█", Style::new().fg(fill))
        } else {
            // 生长底缘:cell 上部 vcov/8 实心 → 反色画下部 (8-vcov)/8 的"背景"。
            (
                crate::render::cells::lower_eighth(8 - vcov),
                Style::new().fg(bg).bg(fill),
            )
        };
        let Ok(y) = u16::try_from(row) else {
            continue;
        };
        for x in full.x..full.right() {
            buf.set_string(x, y, glyph, style);
        }
    }
}

/// SlideInRight 的进退场空壳:右缘锚定,宽度按 `scale`(千分比)自右向左滑入 / 滑出,
/// 生长的左缘用 1/8 cell 八分块平滑(反色补齐:cell 右部实心、左部露背景)。
fn draw_h_grow_shell(frame: &mut Frame<'_>, full: Rect, scale: u16, theme: &Theme) {
    if full.width == 0 || full.height == 0 {
        return;
    }
    let full_w_e = u32::from(full.width) * 8;
    let cur_w_e = full_w_e * u32::from(scale) / 1000;
    if cur_w_e < 8 {
        return;
    }
    let right_e = u32::from(full.right()) * 8;
    let left_e = right_e.saturating_sub(cur_w_e);
    let col0 = left_e / 8;
    let x0 = u16::try_from(col0).unwrap_or(full.x);
    frame.render_widget(
        Clear,
        Rect::new(x0, full.y, full.right().saturating_sub(x0), full.height),
    );

    let fill = shell_fill(scale, theme);
    let bg = theme.base;
    let buf = frame.buffer_mut();
    for col in col0..u32::from(full.right()) {
        let c_lo = col * 8;
        let hcov = right_e.min(c_lo + 8).saturating_sub(left_e.max(c_lo));
        if hcov == 0 {
            continue;
        }
        let (glyph, style) = if hcov >= 8 {
            ("█", Style::new().fg(fill))
        } else {
            // 生长左缘:cell 右部 hcov/8 实心 → 反色画左部 (8-hcov)/8 的"背景"。
            (
                crate::render::cells::left_eighth(8 - hcov),
                Style::new().fg(bg).bg(fill),
            )
        };
        let Ok(x) = u16::try_from(col) else {
            continue;
        };
        for y in full.y..full.bottom() {
            buf.set_string(x, y, glyph, style);
        }
    }
}

/// 边框线符集(Rounded):倒计时蔓延只重染这些 cell,标题 / 底边提示等
/// 画在边框上的文字保持原色可读。
fn is_border_glyph(symbol: &str) -> bool {
    matches!(symbol, "─" | "│" | "╭" | "╮" | "╰" | "╯")
}

/// 带 TTL 卡片的边框倒计时:暗色(骨架色 surface1)自左上角沿两条路径同时蔓延
/// —— 顺时针经顶边 → 右边、逆时针经左边 → 底边 —— 于右下角汇合熄灭,扫一眼
/// 边框即知剩余时长。前沿不是硬边,而是一条占周长 1/4 的渐变带:级别色到
/// surface1 的色距大,若只在前沿单格 lerp,每格会在 ttl/周长 的瞬间完成整段
/// 变色,肉眼就是逐格跳变;空间上拉开梯度后,每格的变暗被摊到整条带通过的时长。
///
/// # Params:
///   - `buf`: 已画好边框的帧缓冲
///   - `area`: 卡片矩形(宽高 ≥ 2,调用方已 guard)
///   - `progress`: TTL 消耗进度(千分比;0 不染,1000 全暗)
///   - `bright`: 边框亮色(级别色,蔓延的起点色)
///   - `theme`: 配色(暗端取 surface1)
fn decay_border(
    buf: &mut ratatui::buffer::Buffer,
    area: Rect,
    progress: u16,
    bright: Color,
    theme: &Theme,
) {
    if area.width < 2 || area.height < 2 {
        return;
    }
    // 单条路径长(cell 数):左上角 d=0,右下角 d=len,两条路径等长。
    let len = u64::from(area.width - 1) + u64::from(area.height - 1);
    // 渐变带宽(千分 cell):周长的 1/4,至少 2 格。
    let band = (len * 1000 / 4).max(2000);
    // 前沿扫过 len*1000 + band:progress=1000 时带尾越过右下角,整圈才真正全暗。
    let front = u64::from(progress.min(1000)) * (len * 1000 + band) / 1000;
    let mut paint = |x: u16, y: u16, d: u64| {
        let cov = front.saturating_sub(d * 1000).min(band);
        if cov == 0 {
            return;
        }
        if let Some(cell) = buf.cell_mut((x, y))
            && is_border_glyph(cell.symbol())
        {
            cell.set_fg(crate::render::color::lerp_color(
                bright,
                theme.surface1,
                cov,
                /*denom*/ band,
            ));
        }
    };
    let (top, bottom) = (area.y, area.bottom().saturating_sub(1));
    let (left, right) = (area.x, area.right().saturating_sub(1));
    for x in left..=right {
        // 顶边(含两上角):顺时针路径起始段。
        paint(x, top, u64::from(x - left));
    }
    for y in top + 1..=bottom {
        // 右边(含右下角)接顶边之后;左边(含左下角)是逆时针路径起始段。
        paint(right, y, u64::from(area.width - 1) + u64::from(y - top));
        paint(left, y, u64::from(y - top));
    }
    for x in left + 1..right {
        // 底边内段接左边之后,向右与右下角汇合。
        paint(x, bottom, u64::from(area.height - 1) + u64::from(x - left));
    }
}

/// 空壳体色:随 `scale` 从骨架色 surface1 渐变到卡片体色 base,
/// 到位(scale→1000)时与真卡片背景一致,切换不突变。
fn shell_fill(scale: u16, theme: &Theme) -> Color {
    crate::render::color::lerp_color(
        theme.surface1,
        theme.base,
        u64::from(scale),
        /*den*/ 1000,
    )
}

/// 级别 → 标题前缀符号(Normal 无符号)。
fn symbol(tint: TextTint) -> &'static str {
    match tint {
        TextTint::Normal => "",
        TextTint::Warn => "⚠ ",
        TextTint::Error => "✗ ",
    }
}

/// [`symbol`] 的显示宽度(量宽用,与渲染严格同源)。
fn symbol_width(tint: TextTint) -> u16 {
    text_width(symbol(tint))
}

/// 字符串显示宽度(unicode,CJK 计 2),u16 饱和。
fn text_width(s: &str) -> u16 {
    u16::try_from(UnicodeWidthStr::width(s)).unwrap_or(u16::MAX)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::layout::Rect;

    use mineral_protocol::{SpanAlign, SpanFg, TextSpan};

    use super::{Card, CardMotion, CardTtl, plain_body, plain_line};
    use crate::components::toast::notifications::TextTint;
    use crate::render::theme::Theme;

    /// 与 default.lua 默认一致的动画拍数(96ms ÷ 16ms)。
    const ANIM_TICKS: u16 = 6;

    /// 一张 warn 卡(标题 + 两行纯文本 body)。
    fn warn_card() -> Card {
        Card::new(
            TextTint::Warn,
            plain_line("v0.9.0 要点"),
            plain_body(vec![
                "新增配置 toast.position".to_owned(),
                "旧键 search.style 改名".to_owned(),
            ]),
            /*id*/ None,
            ANIM_TICKS,
            /*ttl*/ None,
        )
    }

    /// 渲染一张卡到 80x10 的内存终端。
    fn draw(card: &Card, motion: CardMotion) -> color_eyre::Result<Terminal<TestBackend>> {
        let theme = Theme::default();
        let mut t = Terminal::new(TestBackend::new(80, 10))?;
        let w = card.width("x");
        let h = card.height();
        t.draw(|f| {
            let area = f.area();
            // 右上角第 2 行起的 slot(与 immersive 锚点一致;ExpandDown 用同一 slot 便于对照)。
            let slot = Rect::new(area.width.saturating_sub(w + 1), 1, w, h);
            card.render(f, slot, motion, "x", &theme, Instant::now());
        })?;
        Ok(t)
    }

    /// 稳态(完全展开):圆角边框 + ⚠ 标题 + 两行 body + 右下关闭提示。
    #[test]
    fn card_steady_snapshot() -> color_eyre::Result<()> {
        let mut card = warn_card();
        for _ in 0..8 {
            card.tick();
        }
        let t = draw(&card, CardMotion::ExpandDown)?;
        crate::test_support::assert_snap!(
            "驻留卡片稳态:⚠ 标题边框 + 两行 body + 底边 x 关闭提示",
            t.backend()
        );
        Ok(())
    }

    /// ExpandDown 进场中途:1/8 cell 空壳从顶边向下长出,底缘是反色八分块,
    /// 无边框无内容。
    #[test]
    fn card_midexpand_snapshot() -> color_eyre::Result<()> {
        let mut card = warn_card();
        for _ in 0..4 {
            card.tick();
        }
        let t = draw(&card, CardMotion::ExpandDown)?;
        crate::test_support::assert_snap!(
            "卡片 ExpandDown 中途:纯色空壳自顶向下长出,底缘八分块平滑",
            t.backend()
        );
        Ok(())
    }

    /// SlideInRight 进场中途:1/8 cell 空壳右缘锚定向左滑入,左缘是反色八分块。
    #[test]
    fn card_midslide_snapshot() -> color_eyre::Result<()> {
        let mut card = warn_card();
        for _ in 0..5 {
            card.tick();
        }
        let t = draw(&card, CardMotion::SlideInRight)?;
        crate::test_support::assert_snap!(
            "卡片 SlideInRight 中途:纯色空壳自右缘向左滑入,左缘八分块平滑",
            t.backend()
        );
        Ok(())
    }

    /// dismiss 后动画收完进入 dormant;refresh 复活退场中的卡。
    #[test]
    fn dismiss_then_dormant_and_refresh_revives() -> color_eyre::Result<()> {
        let mut card = warn_card();
        for _ in 0..8 {
            card.tick();
        }
        assert!(!card.dormant(), "稳态不该休眠");
        card.dismiss();
        assert!(card.leaving(), "dismiss 后应在退场中");
        for _ in 0..3 {
            card.tick();
        }
        card.refresh(
            TextTint::Error,
            plain_line("新标题"),
            plain_body(vec!["x".to_owned()]),
            /*ttl*/ None,
        );
        assert!(!card.leaving(), "refresh 应复活退场中的卡");
        card.dismiss();
        for _ in 0..8 {
            card.tick();
        }
        assert!(card.dormant(), "退场归零后应休眠");
        Ok(())
    }

    /// 标题 spans 落色:无样式 span 用级别色(accent),带 fg 的 span 覆盖为指定色。
    #[test]
    fn styled_title_spans_color_cells() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut title = plain_line("ab");
        let mut styled = TextSpan::plain("cd");
        styled.fg = Some(SpanFg::Green);
        title.push(styled);
        let mut card = Card::new(
            TextTint::Normal,
            title,
            plain_body(vec!["body".to_owned()]),
            /*id*/ None,
            ANIM_TICKS,
            /*ttl*/ None,
        );
        for _ in 0..8 {
            card.tick();
        }
        let mut t = Terminal::new(TestBackend::new(20, 5))?;
        t.draw(|f| {
            card.render(
                f,
                Rect::new(0, 0, 12, 3),
                CardMotion::ExpandDown,
                /*close_hint*/ "",
                &theme,
                /*now*/ Instant::now(),
            );
        })?;
        let buf = t.backend().buffer();
        // 上边框标题:x=0 是圆角,x=1 空格,x=2 起 "ab" "cd"。
        let plain = buf
            .cell((2, 0))
            .ok_or_else(|| color_eyre::eyre::eyre!("标题 plain cell 缺失"))?;
        assert_eq!(plain.fg, theme.accent, "无样式标题 span 用级别色");
        let styled = buf
            .cell((4, 0))
            .ok_or_else(|| color_eyre::eyre::eyre!("标题 styled cell 缺失"))?;
        assert_eq!(styled.fg, theme.green, "带 fg 的标题 span 覆盖级别色");
        Ok(())
    }

    /// 行内三段布局:同一行 left / center / right 三段各自对齐
    /// (`|xxxx  xxx  xxxx|`),量宽含段间隙。
    #[test]
    fn line_with_three_aligned_groups_snapshot() -> color_eyre::Result<()> {
        /// 指定段位的纯文本 span。
        fn span_at(text: &str, align: SpanAlign) -> TextSpan {
            let mut s = TextSpan::plain(text);
            s.align = align;
            s
        }
        let mut card = Card::new(
            TextTint::Normal,
            plain_line("对齐"),
            vec![
                vec![
                    span_at("左左", SpanAlign::Left),
                    span_at("中", SpanAlign::Center),
                    span_at("右右", SpanAlign::Right),
                ],
                vec![span_at("满宽参照行—————————", SpanAlign::Left)],
            ],
            /*id*/ None,
            ANIM_TICKS,
            /*ttl*/ None,
        );
        for _ in 0..8 {
            card.tick();
        }
        let t = draw(&card, CardMotion::ExpandDown)?;
        crate::test_support::assert_snap!("卡片行内三段布局:左贴左、中居中、右贴右", t.backend());
        Ok(())
    }

    /// 量宽:CJK body 最宽行决定卡宽(行宽 + 边框 2 + padding 2)。
    #[test]
    fn width_covers_widest_cjk_line() -> color_eyre::Result<()> {
        let card = Card::new(
            TextTint::Normal,
            plain_line("t"),
            plain_body(vec!["中文宽度测试".to_owned(), "ascii".to_owned()]),
            /*id*/ None,
            ANIM_TICKS,
            /*ttl*/ None,
        );
        // "中文宽度测试" 显示宽 12,+4(边框 2 + padding 2)= 16。
        assert_eq!(card.width(/*close_hint*/ ""), 16);
        // 两行 body + 上下边框。
        assert_eq!(card.height(), 4);
        Ok(())
    }

    /// 样式 spans 落色到 cell:accent+bold span 的字符前景是 accent 且带 BOLD,
    /// 同行纯文本 span 仍是正文色(快照抓不到色,逐 cell 验)。
    #[test]
    fn styled_spans_color_and_modifier_reach_cells() -> color_eyre::Result<()> {
        use ratatui::style::Modifier;
        let theme = Theme::default();
        let mut card = Card::new(
            TextTint::Normal,
            Vec::new(),
            vec![vec![
                TextSpan::plain("ab"),
                TextSpan {
                    text: "cd".to_owned(),
                    fg: Some(SpanFg::Accent),
                    bold: true,
                    italic: false,
                    underline: false,
                    dim: false,
                    align: SpanAlign::Left,
                },
            ]],
            /*id*/ None,
            ANIM_TICKS,
            /*ttl*/ None,
        );
        for _ in 0..8 {
            card.tick();
        }
        let mut t = Terminal::new(TestBackend::new(20, 5))?;
        t.draw(|f| {
            card.render(
                f,
                Rect::new(0, 0, 10, 3),
                CardMotion::ExpandDown,
                /*close_hint*/ "",
                &theme,
                /*now*/ Instant::now(),
            );
        })?;
        let buf = t.backend().buffer();
        // 内容行 y=1,边框 1 + padding 1 → 文本起点 x=2:"abcd"。
        let plain = buf
            .cell((2, 1))
            .ok_or_else(|| color_eyre::eyre::eyre!("纯文本 cell 缺失"))?;
        assert_eq!(plain.fg, theme.text, "纯文本 span 用正文色");
        let styled = buf
            .cell((4, 1))
            .ok_or_else(|| color_eyre::eyre::eyre!("样式 cell 缺失"))?;
        assert_eq!(styled.fg, theme.accent, "accent 角色应落到主题色");
        assert!(
            styled.modifier.contains(Modifier::BOLD),
            "bold 修饰位应到 cell"
        );
        Ok(())
    }

    /// 错误级卡片边框用红色(快照抓不到色,逐 cell 验)。
    #[test]
    fn error_card_border_is_red() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let mut card = Card::new(
            TextTint::Error,
            plain_line("出错"),
            plain_body(vec!["detail".to_owned()]),
            /*id*/ None,
            ANIM_TICKS,
            /*ttl*/ None,
        );
        for _ in 0..8 {
            card.tick();
        }
        let mut t = Terminal::new(TestBackend::new(40, 6))?;
        t.draw(|f| {
            card.render(
                f,
                Rect::new(0, 0, 20, 3),
                CardMotion::ExpandDown,
                /*close_hint*/ "",
                &theme,
                /*now*/ Instant::now(),
            );
        })?;
        let buf = t.backend().buffer();
        let corner = buf.cell((0, 0)).map(|c| c.fg);
        assert_eq!(corner, Some(theme.red), "错误卡边框应为红");
        Ok(())
    }

    /// 带 TTL 卡片边框倒计时蔓延:暗色自左上角沿两条路径(顶边→右边、左边→底边)
    /// 同时推进,右下角最后熄灭。半程时左上半圈已暗(surface1)、右下半圈仍亮,
    /// 前沿是多格渐变带(带内相邻格颜色互异、非端点色),画在边框上的标题文字不被染暗。
    #[test]
    fn ttl_border_decays_from_top_left_toward_bottom_right() -> color_eyre::Result<()> {
        let theme = Theme::default();
        let total = Duration::from_secs(8);
        let born = Instant::now();
        let mut card = Card::new(
            TextTint::Normal,
            plain_line("TT"),
            plain_body(vec!["0123456789".to_owned()]),
            /*id*/ None,
            ANIM_TICKS,
            Some(CardTtl {
                deadline: born + total,
                total,
            }),
        );
        for _ in 0..8 {
            card.tick();
        }
        let mut t = Terminal::new(TestBackend::new(20, 5))?;
        t.draw(|f| {
            // 卡宽 14(body 10 + 4)、高 3;消耗半程:路径长 L = 13 + 2 = 15,
            // 渐变带宽 = L/4 = 3.75 格,前沿(带头)= (15+3.75)/2 = 9.375 格 ——
            // d≤5 全暗、d=6..=9 落在渐变带内、d≥10 未波及。
            card.render(
                f,
                Rect::new(0, 0, 14, 3),
                CardMotion::ExpandDown,
                /*close_hint*/ "",
                &theme,
                /*now*/ born + total / 2,
            );
        })?;
        let buf = t.backend().buffer();
        let fg = |x: u16, y: u16| buf.cell((x, y)).map(|c| c.fg);
        assert_eq!(fg(0, 0), Some(theme.surface1), "左上角(d=0)应已熄灭");
        assert_eq!(
            fg(0, 2),
            Some(theme.surface1),
            "左下角(d=2)在逆时针路径上,应已熄灭"
        );
        assert_eq!(fg(13, 0), Some(theme.accent), "右上角(d=13)未被波及,仍亮");
        assert_eq!(fg(13, 2), Some(theme.accent), "右下角(d=15)最后熄灭,仍亮");
        let band_a = fg(6, 0).ok_or_else(|| color_eyre::eyre::eyre!("渐变带 cell 缺失"))?;
        let band_b = fg(7, 0).ok_or_else(|| color_eyre::eyre::eyre!("渐变带 cell 缺失"))?;
        for (d, c) in [(6, band_a), (7, band_b)] {
            assert!(
                c != theme.accent && c != theme.surface1,
                "带内格(d={d})应是亮暗之间的 lerp 中间色,实得 {c:?}"
            );
        }
        assert!(
            band_a != band_b,
            "渐变带内相邻格应深浅递进(不是单格硬边),实得 {band_a:?} == {band_b:?}"
        );
        // 标题画在顶边已被蔓延扫过的区段,文字 cell 不该被染暗。
        let title_cell = buf
            .cell((2, 0))
            .ok_or_else(|| color_eyre::eyre::eyre!("标题 cell 缺失"))?;
        assert_eq!(title_cell.symbol(), "T", "x=2 应是标题首字符");
        assert_eq!(title_cell.fg, theme.accent, "标题文字不参与边框蔓延");
        Ok(())
    }
}

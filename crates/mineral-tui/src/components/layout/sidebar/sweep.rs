//! 左栏 Playlists ↔ Library 切换的横向过渡合成。
//!
//! 两个视图各渲染到一块和左栏等大的离屏 [`Buffer`],再按过渡进度把列搬运 / 拼接进目标
//! 区域。`Push` 让两块一起平移、`Cover` 让新视图从右覆盖旧视图。过渡风格由配置
//! `tui.animation.view_sweep` 选定([`SweepStyle`],调用方从 `state.cfg` 取传入)。

use mineral_config::SweepStyle;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::{library, playlists};
use crate::render::theme::Theme;
use crate::runtime::state::AppState;

/// 缓动进度满值(千分比),对齐 [`crate::render::anim::Transition::eased`]。
const FULL: u32 = 1000;

/// 把 Playlists 与 Library 两视图按 `eased`(缓动千分比)横向合成到 `area`。
///
/// 仅在过渡中途调用(进度既非起点也非终点);端点退化为单视图由 [`super::draw`] 直接画。
///
/// # Params:
///   - `buf`: 目标(屏幕)缓冲
///   - `area`: 左栏区域
///   - `state`: 应用状态(两视图渲染所需)
///   - `theme`: 主题
///   - `eased`: 缓动后的过渡进度,`0` = 全 Playlists、满值 = 全 Library
///   - `style`: 过渡风格(Push / Cover)
pub fn draw(
    buf: &mut Buffer,
    area: Rect,
    state: &AppState,
    theme: &Theme,
    eased: u16,
    style: SweepStyle,
) {
    // 两视图各渲染到等大离屏 buffer(坐标系与屏幕一致,含 area 的 x/y 偏移)。
    let mut pl = Buffer::empty(area);
    let mut lib = Buffer::empty(area);
    playlists::render_to(&mut pl, area, state, theme);
    library::render_to(&mut lib, area, state, theme);

    let w = area.width;
    // Library 已「进入」的列数(0..=w)。
    let advance = u16::try_from(u32::from(w) * u32::from(eased) / FULL)
        .unwrap_or(w)
        .min(w);

    for c in 0..w {
        let (src, src_c) = match style {
            // 旧视图不动;新视图占据最右 advance 列(其左边框落在分界列 = 覆盖左缘)。
            SweepStyle::Cover => {
                let split = w - advance;
                if c < split {
                    (&pl, c)
                } else {
                    (&lib, c - split)
                }
            }
            // 旧视图整体左移 advance,新视图从右补入。
            // SweepStyle 是 #[non_exhaustive]:未来新风格接线前按 Push 兜底。
            SweepStyle::Push | _ => {
                if c + advance < w {
                    (&pl, c + advance)
                } else {
                    (&lib, c + advance - w)
                }
            }
        };
        copy_col(buf, area, src, c, src_c);
    }
}

/// 把离屏 `src` 的第 `src_c` 列(相对 `area`)整列搬到目标 `dst` 的第 `dst_c` 列。
///
/// # Params:
///   - `dst`: 目标缓冲
///   - `area`: 列所在区域(提供 x/y 偏移与行高)
///   - `src`: 离屏源缓冲
///   - `dst_c`: 目标相对列号
///   - `src_c`: 源相对列号
fn copy_col(dst: &mut Buffer, area: Rect, src: &Buffer, dst_c: u16, src_c: u16) {
    let dx = area.x + dst_c;
    let sx = area.x + src_c;
    for ry in area.y..area.y.saturating_add(area.height) {
        if let Some(cell) = src.cell((sx, ry)) {
            let cell = cell.clone();
            if let Some(slot) = dst.cell_mut((dx, ry)) {
                *slot = cell;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::render::anim::Transition;
    use crate::render::theme::Theme;
    use crate::runtime::state::View;
    use mineral_config::SweepStyle;

    /// 把一帧 sweep 合成画到 `TestBackend` 并返回其快照串。
    fn render_sweep(eased: u16, style: SweepStyle) -> color_eyre::Result<String> {
        let mut state = crate::test_support::state_with_tracks()?;
        // sweep 同屏要两视图都有内容:playlists 与选中歌单的 tracks。
        state.browse.view.switch_to(View::Library);
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        t.draw(|f| {
            let area = f.area();
            super::draw(
                f.buffer_mut(),
                area,
                &state,
                &Theme::default(),
                eased,
                style,
            );
        })?;
        Ok(format!("{:?}", t.backend()))
    }

    /// Push 中途帧:歌单左移、曲目从右补入,两块同屏。
    #[test]
    fn push_midframe_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let mut state = crate::test_support::state_with_tracks()?;
        state.browse.view.switch_to(View::Library);
        t.draw(|f| {
            let area = f.area();
            super::draw(
                f.buffer_mut(),
                area,
                &state,
                &Theme::default(),
                /*eased*/ 500,
                SweepStyle::Push,
            );
        })?;
        crate::test_support::assert_snap!("Push 中途:歌单左移、曲目从右补入", t.backend());
        Ok(())
    }

    /// Cover 中途帧:歌单原地不动,曲目从右覆盖(左缘见 Library 边框竖线)。
    #[test]
    fn cover_midframe_snapshot() -> color_eyre::Result<()> {
        let mut t = Terminal::new(TestBackend::new(40, 12))?;
        let mut state = crate::test_support::state_with_tracks()?;
        state.browse.view.switch_to(View::Library);
        t.draw(|f| {
            let area = f.area();
            super::draw(
                f.buffer_mut(),
                area,
                &state,
                &Theme::default(),
                /*eased*/ 500,
                SweepStyle::Cover,
            );
        })?;
        crate::test_support::assert_snap!("Cover 中途:曲目从右覆盖、左缘竖线", t.backend());
        Ok(())
    }

    /// 端点退化:eased=0 全是 Playlists、eased=满值全是 Library,且与对应单视图一致;
    /// 两种 style 在端点结果相同(无中途叠加)。
    #[test]
    fn endpoints_degenerate_to_single_view() -> color_eyre::Result<()> {
        let state = crate::test_support::state_with_tracks()?;
        // eased=0:渲染 = 纯 Playlists(与 playlists::render_to 一致)。
        let mut tp = Terminal::new(TestBackend::new(40, 12))?;
        tp.draw(|f| {
            let area = f.area();
            super::playlists::render_to(f.buffer_mut(), area, &state, &Theme::default());
        })?;
        let pure_pl = format!("{:?}", tp.backend());
        assert_eq!(
            render_sweep(0, SweepStyle::Push)?,
            pure_pl,
            "eased=0 应等于纯 Playlists"
        );
        assert_eq!(
            render_sweep(0, SweepStyle::Cover)?,
            pure_pl,
            "Cover 端点同样退化"
        );

        // eased=满值:渲染 = 纯 Library。
        let mut tl = Terminal::new(TestBackend::new(40, 12))?;
        let mut lib_state = crate::test_support::state_with_tracks()?;
        lib_state.browse.view.switch_to(View::Library);
        tl.draw(|f| {
            let area = f.area();
            super::library::render_to(f.buffer_mut(), area, &lib_state, &Theme::default());
        })?;
        let pure_lib = format!("{:?}", tl.backend());
        assert_eq!(
            render_sweep(1000, SweepStyle::Push)?,
            pure_lib,
            "eased=满应等于纯 Library"
        );
        assert_eq!(
            render_sweep(1000, SweepStyle::Cover)?,
            pure_lib,
            "Cover 端点同样退化"
        );
        Ok(())
    }

    /// 打断反向:enter 到一半再 leave,缓动进度从当前值单调回落、不跳变(几何连续)。
    #[test]
    fn interrupt_reverse_is_continuous() {
        let mut vp = Transition::new(18);
        vp.enter();
        for _ in 0..6 {
            vp.tick();
        }
        let mid = vp.eased();
        assert!(mid > 0 && mid < 1000, "应处于中途: {mid}");
        // 中途反向:不重置,继续从当前进度朝 0 走。
        vp.leave();
        assert_eq!(vp.eased(), mid, "反向瞬间不跳变");
        let mut prev = mid;
        for _ in 0..18 {
            vp.tick();
            let v = vp.eased();
            assert!(v <= prev, "反向应单调回落: {v} <= {prev}");
            prev = v;
        }
        assert!(vp.at_min(), "最终回到 Playlists 端点");
    }
}

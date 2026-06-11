//! 左栏视图切换:Playlists ↔ Library 两态 + 横向过渡,逻辑视图由内部 [`Toggle`] 派生。
//!
//! 与 [`Toggle`] 同思路——逻辑态(供按键路由 / 选中语义)与渲染过渡位置(供 sidebar
//! 端点退化 / 中途 sweep)由同一 [`Transition`](crate::render::anim::Transition) 表达,
//! 消除「`View` 标志与动画目标手动同步」的冗余。

use crate::render::anim::Toggle;

use super::View;

/// 左栏视图切换态([`AppState`](crate::runtime::state::AppState) 的视图域)。
///
/// `current()` 给路由 / 选中语义,`at_min` / `at_max` / `eased_in_out` 给渲染。
/// 实现 `PartialEq<View>`,故 `state.view == View::Library` 直接可比,无需显式 `current()`。
#[derive(Clone, Copy, Debug)]
pub struct ViewSwitch(Toggle);

impl ViewSwitch {
    /// 构造一个停在 [`View::Playlists`] 的切换态。`ticks` 为切到 Library 所需拍数。
    pub(crate) fn new(ticks: u16) -> Self {
        Self(Toggle::new(ticks))
    }

    /// 当前逻辑视图(目标方向):`on` = Library、否则 Playlists。立即反映 [`Self::switch_to`]。
    pub fn current(&self) -> View {
        if self.0.on() {
            View::Library
        } else {
            View::Playlists
        }
    }

    /// 切到目标视图并驱动横向过渡(切 Library 朝满值、回 Playlists 朝 `0`);
    /// 中途反向只改目标不跳变。
    pub fn switch_to(&mut self, view: View) {
        self.0.set(view == View::Library);
    }

    /// 推进过渡一拍。
    pub fn tick(&mut self) {
        self.0.tick();
    }

    /// 进度处于 Playlists 端点(`0`):sidebar 直接画 Playlists 单视图(零开销)。
    pub fn at_min(&self) -> bool {
        self.0.at_min()
    }

    /// 进度处于 Library 端点(满值):sidebar 直接画 Library 单视图。
    pub fn at_max(&self) -> bool {
        self.0.at_max()
    }

    /// 当前过渡位置经 ease-in-out 映射的千分比(`0` = Playlists、满值 = Library),喂 sweep。
    pub fn eased_in_out(&self) -> u16 {
        self.0.eased_in_out()
    }
}

impl PartialEq<View> for ViewSwitch {
    fn eq(&self, other: &View) -> bool {
        self.current() == *other
    }
}

//! 浮层展开进度与向上层让渡的高亮程度。

/// 一层浮层在本帧的揭开进度,以及压在它上面那层的进度(千分比,1000 = 完全展开)。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayReveal {
    /// 本层自己的入场进度。
    pub own: u16,

    /// 直接压在本层之上那层的入场进度;上面没有东西时为 0。
    pub above: u16,
}

impl OverlayReveal {
    /// 千分比进度的满值。
    pub const FULL: u16 = 1000;

    /// 焦点让渡给上层的程度——上层揭开到几成,本层的选中高亮就该淡出几成。
    ///
    /// # Return:
    ///   千分比;上面没有浮层时为 0(高亮全亮)。
    #[inline]
    pub fn yielded(self) -> u16 {
        self.above.min(Self::FULL)
    }
}

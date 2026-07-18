//! 已编码封面协议的字节预算 LRU 缓存。
//!
//! 一个 `StatefulProtocol` 内部留着源图副本 + 终端编码序列(kitty transmit / sixel),
//! 全屏大图能到 MB 级。它是渲染加速缓存:编一次、之后每帧只 place。同一封面可在多个
//! 渲染尺寸下**并存**(常规面板与全屏各占一槽)——单槽会让进出全屏来回互踩,每次都
//! 重编码。槽数按 URL 封顶(`insert` 的 `sizes_per_image`),终端 resize 途中的一次性
//! 尺寸自然被滚动替换。全局只保留"屏幕上 + 附近刚滚过"的那些就够——滚远的滚回来时
//! 后台几十毫秒重编即可(其间有 halfblock 真图兜底,不闪不卡)。故按字节封顶、渲染
//! 命中即 touch、越预算逐出最久未渲染者。
//!
//! 每条字节由编码 worker 估算(源像素 + 目标编码尺寸)后随结果带入,本缓存只记账不重算。

use std::cell::RefCell;

use mineral_model::MediaUrl;
use ratatui_image::protocol::StatefulProtocol;
use rustc_hash::FxHashMap;

/// 某封面在一个渲染尺寸下的已编码槽。
struct Slot {
    /// 编码时的目标 cell 尺寸 `(w, h)`,同 URL 内以此区分;与渲染 dims 不一致即未命中。
    dims: (u16, u16),

    /// 已编码的有状态协议(渲染线程 place 时按 `&mut` 复用,不重编码)。
    protocol: StatefulProtocol,

    /// 该协议估算的常驻字节数(源图副本 + 编码序列),记账用。
    bytes: u64,

    /// 上次被渲染命中的单调序号;最小者最久未渲染,优先逐出。
    last_used: u64,

    /// 图数据仍在流式传输途中(kitty transmit backlog 未写完):不参与渲染命中,
    /// 上层继续 halfblock 兜底——占位符绝不指向终端还没收全的图。
    awaiting_transmit: bool,
}

/// 缓存内部可变状态(渲染路径持 `&AppState`,故整体走 `RefCell`)。
struct Inner {
    /// URL → 各渲染尺寸的槽(每尺寸一槽)。
    entries: FxHashMap<MediaUrl, Vec<Slot>>,

    /// 当前占用字节合计(所有 [`Slot::bytes`] 之和)。
    total_bytes: u64,

    /// 单调访问计数器,每次 `render_hit` / `insert` 自增后赋给 `last_used`。
    tick: u64,
}

/// 已编码封面协议缓存:字节预算 LRU,同 URL 多尺寸槽并存。
///
/// 渲染命中即 touch(保护正在显示的协议),`insert` 越预算逐出最久未渲染槽。
/// 协议是可廉价重建的渲染加速物,故逐出无损正确性,只是滚回时短暂走 halfblock 兜底。
pub(crate) struct ProtocolCache {
    /// 内部可变状态。
    inner: RefCell<Inner>,

    /// 字节预算上限(来自配置 `tui.cover.cache.protocol`)。
    budget: u64,
}

impl ProtocolCache {
    /// 建空缓存,字节预算为 `budget`。
    ///
    /// # Params:
    ///   - `budget`: 已编码协议的常驻字节上限
    pub(crate) fn new(budget: u64) -> Self {
        Self {
            inner: RefCell::new(Inner {
                entries: FxHashMap::default(),
                total_bytes: 0,
                tick: 0,
            }),
            budget,
        }
    }

    /// 命中同尺寸协议则 touch 并交渲染闭包 place,返回是否命中。
    ///
    /// dims 无对应槽(没编过 / 已被逐)按未命中处理,交由上层投递重编码。
    ///
    /// # Params:
    ///   - `url`: 封面 URL
    ///   - `dims`: 当前渲染目标 cell 尺寸
    ///   - `render`: 命中时执行的 place 闭包(拿到协议的 `&mut`)
    ///
    /// # Return:
    ///   是否命中并渲染
    pub(crate) fn render_hit(
        &self,
        url: &MediaUrl,
        dims: (u16, u16),
        render: impl FnOnce(&mut StatefulProtocol),
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.tick = inner.tick.wrapping_add(1);
        let tick = inner.tick;
        let Some(slot) = inner
            .entries
            .get_mut(url)
            .and_then(|slots| slots.iter_mut().find(|s| s.dims == dims))
        else {
            return false;
        };
        if slot.awaiting_transmit {
            return false;
        }
        slot.last_used = tick;
        render(&mut slot.protocol);
        true
    }

    /// 是否已缓存该 URL 的**同尺寸**协议。**不** touch(预热探测用,非渲染)。
    pub(crate) fn contains_dims(&self, url: &MediaUrl, dims: (u16, u16)) -> bool {
        matches!(
            self.inner.borrow().entries.get(url),
            Some(slots) if slots.iter().any(|s| s.dims == dims)
        )
    }

    /// 同尺寸协议已缓存**且图数据已送达终端**(本帧 `render_hit` 必命中)。**不** touch。
    /// 与 [`Self::contains_dims`] 的差别:待传输槽算「在」(预热去重)但不算「可渲染」。
    pub(crate) fn ready_for_render(&self, url: &MediaUrl, dims: (u16, u16)) -> bool {
        matches!(
            self.inner.borrow().entries.get(url),
            Some(slots) if slots.iter().any(|s| s.dims == dims && !s.awaiting_transmit)
        )
    }

    /// 装入一条编码好的协议:同 `(url, dims)` 是替换;同 URL 槽数越 `sizes_per_image`
    /// 逐出该 URL 内最久未渲染尺寸;全局越字节预算逐出最久未渲染槽。
    ///
    /// # Params:
    ///   - `url`: 封面 URL(内部 clone 一份作 key)
    ///   - `dims`: 编码所用的目标 cell 尺寸
    ///   - `protocol`: 编码好的协议
    ///   - `bytes`: 该协议估算的常驻字节数(worker 侧算好带入)
    ///   - `sizes_per_image`: 同一封面并存的尺寸槽上限(≥1,现读配置传入)
    ///   - `awaiting_transmit`: 图数据是否仍在流式传输途中(传完前该槽不参与渲染命中)
    pub(crate) fn insert(
        &self,
        url: &MediaUrl,
        dims: (u16, u16),
        protocol: StatefulProtocol,
        bytes: u64,
        sizes_per_image: usize,
        awaiting_transmit: bool,
    ) {
        let inner = &mut *self.inner.borrow_mut();
        inner.tick = inner.tick.wrapping_add(1);
        let last_used = inner.tick;
        let slots = inner.entries.entry(url.clone()).or_default();
        if let Some(slot) = slots.iter_mut().find(|s| s.dims == dims) {
            inner.total_bytes = inner.total_bytes.saturating_sub(slot.bytes);
            slot.protocol = protocol;
            slot.bytes = bytes;
            slot.last_used = last_used;
            slot.awaiting_transmit = awaiting_transmit;
        } else {
            slots.push(Slot {
                dims,
                protocol,
                bytes,
                last_used,
                awaiting_transmit,
            });
        }
        inner.total_bytes = inner.total_bytes.saturating_add(bytes);
        // 同 URL 槽数封顶:刚插入槽 last_used 最新,槽数 ≥ 2 时永远不是受害者。
        while slots.len() > sizes_per_image.max(1) {
            let Some(victim) = slots
                .iter()
                .enumerate()
                .min_by_key(|(_, s)| s.last_used)
                .map(|(i, _)| i)
            else {
                break;
            };
            let removed = slots.swap_remove(victim);
            inner.total_bytes = inner.total_bytes.saturating_sub(removed.bytes);
        }
        Self::evict_over_budget(inner, self.budget, Some((url, dims)));
    }

    /// 标记某槽的图数据已全部送达终端:解除「待传输」,恢复渲染命中。
    /// 槽已被逐出 / 清空则空转(传输字节白写,无正确性影响)。
    pub(crate) fn mark_transmit_done(&self, url: &MediaUrl, dims: (u16, u16)) {
        let mut inner = self.inner.borrow_mut();
        if let Some(slot) = inner
            .entries
            .get_mut(url)
            .and_then(|slots| slots.iter_mut().find(|s| s.dims == dims))
        {
            slot.awaiting_transmit = false;
        }
    }

    /// 移除某 URL 的**全部尺寸槽**(其源图被逐 / 有新图要重建时)。
    pub(crate) fn remove(&self, url: &MediaUrl) {
        let mut inner = self.inner.borrow_mut();
        if let Some(slots) = inner.entries.remove(url) {
            let freed = slots
                .iter()
                .fold(0u64, |acc, s| acc.saturating_add(s.bytes));
            inner.total_bytes = inner.total_bytes.saturating_sub(freed);
        }
    }

    /// 现调字节预算(配置热更):缩小立即逐出最久未渲染槽直到回落,**不清整表**
    /// (被逐出的滚回时后台重编,不损正确性);调大只放宽上限。
    ///
    /// # Params:
    ///   - `budget`: 新预算(字节)
    pub(crate) fn set_budget(&mut self, budget: u64) {
        self.budget = budget;
        let inner = &mut *self.inner.borrow_mut();
        Self::evict_over_budget(inner, budget, /*keep*/ None);
    }

    /// 清空(字号变 / 全屏浮层关闭等须整批重建的场景)。
    pub(crate) fn clear(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.entries.clear();
        inner.total_bytes = 0;
    }

    /// 是否为空(测试 / 断言用)。
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.inner.borrow().entries.is_empty()
    }

    /// 逐出最久未渲染槽直到回落预算内。`keep` 是刚插入槽,永不逐出。
    fn evict_over_budget(inner: &mut Inner, budget: u64, keep: Option<(&MediaUrl, (u16, u16))>) {
        while inner.total_bytes > budget {
            let victim = inner
                .entries
                .iter()
                .flat_map(|(url, slots)| slots.iter().map(move |s| (url, s)))
                .filter(|(url, s)| keep != Some((*url, s.dims)))
                .min_by_key(|(_, s)| s.last_used)
                .map(|(url, s)| (url.clone(), s.dims));
            let Some((victim_url, victim_dims)) = victim else {
                break;
            };
            Self::remove_slot(inner, &victim_url, victim_dims);
        }
    }

    /// 摘除一个 `(url, dims)` 槽并回收字节;该 URL 槽清空则连 key 一并移除。
    fn remove_slot(inner: &mut Inner, url: &MediaUrl, dims: (u16, u16)) {
        let Some(slots) = inner.entries.get_mut(url) else {
            return;
        };
        if let Some(idx) = slots.iter().position(|s| s.dims == dims) {
            let removed = slots.swap_remove(idx);
            inner.total_bytes = inner.total_bytes.saturating_sub(removed.bytes);
        }
        if slots.is_empty() {
            inner.entries.remove(url);
        }
    }
}

#[cfg(test)]
mod tests {
    use image::{DynamicImage, RgbImage};
    use mineral_model::MediaUrl;
    use ratatui_image::picker::Picker;
    use ratatui_image::protocol::StatefulProtocol;

    use super::ProtocolCache;

    /// 造一个 halfblocks 协议(不依赖真实终端探测)。字节由调用方另行指定,与协议无关。
    fn proto() -> StatefulProtocol {
        let image = DynamicImage::ImageRgb8(RgbImage::new(16, 16));
        Picker::from_fontsize((8, 16)).new_resize_protocol(image)
    }

    /// 造第 `n` 张封面 URL。
    fn url(n: u32) -> color_eyre::Result<MediaUrl> {
        Ok(MediaUrl::remote(&format!("https://example.com/{n}.jpg"))?)
    }

    /// 未越预算:全部留驻。
    #[test]
    fn under_budget_keeps_all() -> color_eyre::Result<()> {
        let cache = ProtocolCache::new(/*budget*/ 1_000);
        for n in 0..3 {
            cache.insert(
                &url(n)?,
                (10, 10),
                proto(),
                /*bytes*/ 100,
                /*sizes_per_image*/ 3,
                /*awaiting_transmit*/ false,
            );
        }
        assert!(cache.contains_dims(&url(0)?, (10, 10)));
        assert!(cache.contains_dims(&url(2)?, (10, 10)));
        Ok(())
    }

    /// dims 不一致按未命中:`contains_dims` 假、`render_hit` 不触发闭包。
    #[test]
    fn dims_mismatch_is_miss() -> color_eyre::Result<()> {
        let cache = ProtocolCache::new(/*budget*/ 1_000);
        let u0 = url(0)?;
        cache.insert(
            &u0,
            (10, 10),
            proto(),
            /*bytes*/ 100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );

        assert!(!cache.contains_dims(&u0, (20, 20)), "尺寸不同不算命中");
        let mut rendered = false;
        let hit = cache.render_hit(&u0, (20, 20), |_| rendered = true);
        assert!(!hit, "dims 不一致 render_hit 应返回 false");
        assert!(!rendered, "未命中不应执行渲染闭包");
        Ok(())
    }

    /// 越预算:逐出最久未渲染者,`render_hit` touch 过的受保护。
    #[test]
    fn evicts_least_recently_rendered() -> color_eyre::Result<()> {
        // 每条 100 字节,预算 300 恰容 3 条,第 4 条触发逐 1。
        let cache = ProtocolCache::new(/*budget*/ 300);
        let (u0, u1, u2, u3) = (url(0)?, url(1)?, url(2)?, url(3)?);
        cache.insert(
            &u0,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        cache.insert(
            &u1,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        cache.insert(
            &u2,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );

        // 渲染 u0 → 变最近;此刻最久未渲染是 u1。
        assert!(cache.render_hit(&u0, (10, 10), |_| {}));

        cache.insert(
            &u3,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );

        assert!(!cache.contains_dims(&u1, (10, 10)), "u1 最久未渲染,被逐");
        assert!(cache.contains_dims(&u0, (10, 10)), "u0 被 render_hit 保护");
        assert!(cache.contains_dims(&u3, (10, 10)), "刚插入的 u3 留驻");
        Ok(())
    }

    /// remove / clear 正确回收字节:清空后再插入不受旧账拖累。
    #[test]
    fn remove_and_clear_reclaim_bytes() -> color_eyre::Result<()> {
        let cache = ProtocolCache::new(/*budget*/ 300);
        let (u0, u1, u2) = (url(0)?, url(1)?, url(2)?);
        cache.insert(
            &u0,
            (10, 10),
            proto(),
            300,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        ); // 占满
        cache.remove(&u0);
        assert!(cache.is_empty(), "remove 后为空");

        // 账已清零,再塞满额三条不会因旧 300 立刻逐出。
        cache.insert(
            &u1,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        cache.insert(
            &u2,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        assert!(cache.contains_dims(&u1, (10, 10)));
        assert!(cache.contains_dims(&u2, (10, 10)));

        cache.clear();
        assert!(cache.is_empty(), "clear 后为空");
        Ok(())
    }

    /// 同一 URL 两个尺寸并存(常规面板 + 全屏):互不覆盖,各自命中渲染。
    #[test]
    fn same_url_two_dims_coexist() -> color_eyre::Result<()> {
        let cache = ProtocolCache::new(/*budget*/ 1_000);
        let u = url(0)?;
        cache.insert(
            &u,
            (10, 10),
            proto(),
            /*bytes*/ 100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        cache.insert(
            &u,
            (40, 20),
            proto(),
            /*bytes*/ 100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );

        assert!(cache.contains_dims(&u, (10, 10)), "面板尺寸应保留");
        assert!(cache.contains_dims(&u, (40, 20)), "全屏尺寸应并存");
        assert!(cache.render_hit(&u, (10, 10), |_| {}), "面板尺寸可命中");
        assert!(cache.render_hit(&u, (40, 20), |_| {}), "全屏尺寸可命中");
        Ok(())
    }

    /// 同 URL 尺寸数超上限:逐出该 URL 内最久未渲染的尺寸,其他 URL 不受牵连。
    #[test]
    fn per_url_cap_evicts_oldest_dims() -> color_eyre::Result<()> {
        let cache = ProtocolCache::new(/*budget*/ 10_000);
        let (u, other) = (url(0)?, url(1)?);
        cache.insert(
            &other,
            (5, 5),
            proto(),
            100,
            /*sizes_per_image*/ 2,
            /*awaiting_transmit*/ false,
        );
        cache.insert(
            &u,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 2,
            /*awaiting_transmit*/ false,
        );
        cache.insert(
            &u,
            (20, 20),
            proto(),
            100,
            /*sizes_per_image*/ 2,
            /*awaiting_transmit*/ false,
        );
        // 渲染 (10,10) → 变最近;此刻该 URL 内最久未渲染的是 (20,20)。
        assert!(cache.render_hit(&u, (10, 10), |_| {}));

        cache.insert(
            &u,
            (30, 30),
            proto(),
            100,
            /*sizes_per_image*/ 2,
            /*awaiting_transmit*/ false,
        );

        assert!(
            !cache.contains_dims(&u, (20, 20)),
            "同 URL 最久未渲染尺寸被逐"
        );
        assert!(cache.contains_dims(&u, (10, 10)), "刚渲染过的尺寸保留");
        assert!(cache.contains_dims(&u, (30, 30)), "新尺寸留驻");
        assert!(
            cache.contains_dims(&other, (5, 5)),
            "其他 URL 不受同 URL 限额牵连"
        );
        Ok(())
    }

    /// remove 清掉该 URL 全部尺寸并回收字节。
    #[test]
    fn remove_clears_all_dims() -> color_eyre::Result<()> {
        let cache = ProtocolCache::new(/*budget*/ 200);
        let u = url(0)?;
        cache.insert(
            &u,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        cache.insert(
            &u,
            (20, 20),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        cache.remove(&u);
        assert!(cache.is_empty(), "remove 后所有尺寸清空");

        // 字节账清零:预算 200 再装两条 100 不触发逐出。
        cache.insert(
            &u,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        cache.insert(
            &u,
            (20, 20),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        assert!(cache.contains_dims(&u, (10, 10)));
        assert!(cache.contains_dims(&u, (20, 20)));
        Ok(())
    }

    /// 待传输槽:`contains_dims` 真(预热去重照常)但 `render_hit` 假(halfblock 兜底),
    /// 标记传完后恢复命中。
    #[test]
    fn awaiting_transmit_gates_render_until_done() -> color_eyre::Result<()> {
        let cache = ProtocolCache::new(/*budget*/ 1_000);
        let u = url(0)?;
        cache.insert(
            &u,
            (10, 10),
            proto(),
            /*bytes*/ 100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ true,
        );

        assert!(
            cache.contains_dims(&u, (10, 10)),
            "待传输槽存在,预热不应重复投递"
        );
        let mut rendered = false;
        assert!(
            !cache.render_hit(&u, (10, 10), |_| rendered = true),
            "传输途中不参与渲染命中"
        );
        assert!(!rendered, "传输途中不应执行渲染闭包");

        cache.mark_transmit_done(&u, (10, 10));
        assert!(
            cache.render_hit(&u, (10, 10), |_| rendered = true),
            "传完恢复命中"
        );
        assert!(rendered, "传完应执行渲染闭包");

        // 槽已逐出后标记传完:空转不 panic。
        cache.remove(&u);
        cache.mark_transmit_done(&u, (10, 10));
        Ok(())
    }

    /// 同 (URL, 尺寸) 重复插入是替换:字节不重复记账。
    #[test]
    fn same_dims_reinsert_replaces_no_double_count() -> color_eyre::Result<()> {
        let cache = ProtocolCache::new(/*budget*/ 250);
        let (u0, u1) = (url(0)?, url(1)?);
        cache.insert(
            &u0,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        cache.insert(
            &u0,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        // 替换后总账应为 100;再入 100 合计 200 ≤ 250,谁都不该被逐。
        cache.insert(
            &u1,
            (10, 10),
            proto(),
            100,
            /*sizes_per_image*/ 3,
            /*awaiting_transmit*/ false,
        );
        assert!(
            cache.contains_dims(&u0, (10, 10)),
            "重复插入不应虚增字节导致逐出"
        );
        assert!(cache.contains_dims(&u1, (10, 10)));
        Ok(())
    }
}

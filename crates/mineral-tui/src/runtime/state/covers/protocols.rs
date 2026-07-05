//! 已编码封面协议的字节预算 LRU 缓存。
//!
//! 一个 `StatefulProtocol` 内部留着源图副本 + 终端编码序列(kitty transmit / sixel),
//! 全屏大图能到 MB 级。它是渲染加速缓存:编一次、之后每帧只 place。但只保留"屏幕上 +
//! 附近刚滚过"的那些就够——滚远的滚回来时后台几十毫秒重编即可(其间有 halfblock 真图
//! 兜底,不闪不卡)。故按字节封顶、渲染命中即 touch、越预算逐出最久未渲染者。
//!
//! 每条字节由编码 worker 估算(源像素 + 目标编码尺寸)后随结果带入,本缓存只记账不重算。

use std::cell::RefCell;

use mineral_model::MediaUrl;
use ratatui_image::protocol::StatefulProtocol;
use rustc_hash::FxHashMap;

/// 一条已编码协议项。
struct Entry {
    /// 已编码的有状态协议(渲染线程 place 时按 `&mut` 复用,不重编码)。
    protocol: StatefulProtocol,

    /// 编码时的目标 cell 尺寸 `(w, h)`;与当前渲染 dims 不一致即视为未命中、重建。
    dims: (u16, u16),

    /// 该协议估算的常驻字节数(源图副本 + 编码序列),记账用。
    bytes: u64,

    /// 上次被渲染命中的单调序号;最小者最久未渲染,优先逐出。
    last_used: u64,
}

/// 缓存内部可变状态(渲染路径持 `&AppState`,故整体走 `RefCell`)。
struct Inner {
    /// URL → 协议项。
    entries: FxHashMap<MediaUrl, Entry>,

    /// 当前占用字节合计(所有 `Entry::bytes` 之和)。
    total_bytes: u64,

    /// 单调访问计数器,每次 `render_hit` / `insert` 自增后赋给 `last_used`。
    tick: u64,
}

/// 已编码封面协议缓存:字节预算 LRU。
///
/// 渲染命中即 touch(保护正在显示的协议),`insert` 越预算逐出最久未渲染者。
/// 协议是可廉价重建的渲染加速物,故逐出无损正确性,只是滚回时短暂走 halfblock 兜底。
pub(crate) struct ProtocolCache {
    /// 内部可变状态。
    inner: RefCell<Inner>,

    /// 字节预算上限(来自配置 `cache.cover_protocol_memory`)。
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
    /// dims 不一致(字号 / 终端变过)按未命中处理,交由上层投递重编码。
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
        let Some(entry) = inner.entries.get_mut(url) else {
            return false;
        };
        if entry.dims != dims {
            return false;
        }
        entry.last_used = tick;
        render(&mut entry.protocol);
        true
    }

    /// 是否已缓存该 URL 的**同尺寸**协议。**不** touch(预热探测用,非渲染)。
    pub(crate) fn contains_dims(&self, url: &MediaUrl, dims: (u16, u16)) -> bool {
        matches!(self.inner.borrow().entries.get(url), Some(entry) if entry.dims == dims)
    }

    /// 装入一条编码好的协议,越预算则逐出最久未渲染者。
    ///
    /// # Params:
    ///   - `url`: 封面 URL(内部 clone 一份作 key)
    ///   - `dims`: 编码所用的目标 cell 尺寸
    ///   - `protocol`: 编码好的协议
    ///   - `bytes`: 该协议估算的常驻字节数(worker 侧算好带入)
    pub(crate) fn insert(
        &self,
        url: &MediaUrl,
        dims: (u16, u16),
        protocol: StatefulProtocol,
        bytes: u64,
    ) {
        let mut inner = self.inner.borrow_mut();
        inner.tick = inner.tick.wrapping_add(1);
        let last_used = inner.tick;
        if let Some(old) = inner.entries.insert(
            url.clone(),
            Entry {
                protocol,
                dims,
                bytes,
                last_used,
            },
        ) {
            inner.total_bytes = inner.total_bytes.saturating_sub(old.bytes);
        }
        inner.total_bytes = inner.total_bytes.saturating_add(bytes);
        Self::evict_over_budget(&mut inner, self.budget, url);
    }

    /// 移除某 URL 的协议(其源图被逐 / 有新图要重建时)。
    pub(crate) fn remove(&self, url: &MediaUrl) {
        let mut inner = self.inner.borrow_mut();
        if let Some(entry) = inner.entries.remove(url) {
            inner.total_bytes = inner.total_bytes.saturating_sub(entry.bytes);
        }
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

    /// 逐出最久未渲染项直到回落预算内。`keep` 是刚插入项,永不逐出。
    fn evict_over_budget(inner: &mut Inner, budget: u64, keep: &MediaUrl) {
        while inner.total_bytes > budget {
            let victim = inner
                .entries
                .iter()
                .filter(|(url, _)| *url != keep)
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(url, _)| url.clone());
            let Some(victim) = victim else {
                break;
            };
            if let Some(entry) = inner.entries.remove(&victim) {
                inner.total_bytes = inner.total_bytes.saturating_sub(entry.bytes);
            }
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
            cache.insert(&url(n)?, (10, 10), proto(), /*bytes*/ 100);
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
        cache.insert(&u0, (10, 10), proto(), /*bytes*/ 100);

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
        cache.insert(&u0, (10, 10), proto(), 100);
        cache.insert(&u1, (10, 10), proto(), 100);
        cache.insert(&u2, (10, 10), proto(), 100);

        // 渲染 u0 → 变最近;此刻最久未渲染是 u1。
        assert!(cache.render_hit(&u0, (10, 10), |_| {}));

        cache.insert(&u3, (10, 10), proto(), 100);

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
        cache.insert(&u0, (10, 10), proto(), 300); // 占满
        cache.remove(&u0);
        assert!(cache.is_empty(), "remove 后为空");

        // 账已清零,再塞满额三条不会因旧 300 立刻逐出。
        cache.insert(&u1, (10, 10), proto(), 100);
        cache.insert(&u2, (10, 10), proto(), 100);
        assert!(cache.contains_dims(&u1, (10, 10)));
        assert!(cache.contains_dims(&u2, (10, 10)));

        cache.clear();
        assert!(cache.is_empty(), "clear 后为空");
        Ok(())
    }
}

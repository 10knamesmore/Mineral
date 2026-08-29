//! 可直接写入终端 cell buffer 的图片成品字节预算 LRU。
//!
//! 同一实现由独立实例分别承载协议无关 preview 与当前 terminal backend 成品。Kitty 源图片
//! 只按图片身份缓存一次；preview、Sixel、iTerm2 与 halfblocks 按目标像素尺寸并存。
//!
//! 每条字节由编码 worker 估算(源像素 + 目标编码尺寸)后随结果带入,本缓存只记账不重算。

use std::cell::RefCell;

use rustc_hash::FxHashMap;

use crate::image::key::{ImageIdentity, TerminalImageKey};
use crate::image::terminal::TerminalImage;

/// 某图片身份的一条终端成品槽。
struct Slot {
    /// 源图片或 rasterized 成品的缓存键。
    key: TerminalImageKey,

    /// 图片引擎自己的终端图片成品。
    image: TerminalImage,

    /// 编码 worker 给出的终端成品常驻字节估算，记账用。
    bytes: u64,

    /// 上次被渲染命中的单调序号;最小者最久未渲染,优先逐出。
    last_used: u64,
}

/// 缓存内部可变状态(渲染路径持 `&AppState`,故整体走 `RefCell`)。
struct Inner {
    /// 图片身份 → 源图片或各 rasterized 尺寸的槽。
    entries: FxHashMap<ImageIdentity, Vec<Slot>>,

    /// 当前占用字节合计(所有 [`Slot::bytes`] 之和)。
    total_bytes: u64,

    /// 单调访问计数器,每次 `render_if_ready` / `insert` 自增后赋给 `last_used`。
    tick: u64,
}

/// 终端图片成品缓存：字节预算 LRU。
///
/// 渲染命中即 touch(保护正在显示的协议),`insert` 越预算逐出最久未渲染槽。
/// 协议是可廉价重建的渲染加速物,故逐出无损正确性,只是滚回时短暂走 halfblock 兜底。
pub(crate) struct TerminalImageCache {
    /// 内部可变状态。
    inner: RefCell<Inner>,

    /// 字节预算上限，由所属 preview 或 terminal cache 配置提供。
    budget: u64,
}

impl TerminalImageCache {
    /// 建空缓存,字节预算为 `budget`。
    ///
    /// # Params:
    ///   - `budget`: 终端图片成品的常驻字节上限
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

    /// 命中终端图片键则 touch 并交渲染闭包显示。
    ///
    /// # Params:
    ///   - `key`: 源图片或 rasterized 成品身份
    ///   - `render`: 命中时执行的 place 闭包(拿到协议的 `&mut`)
    ///
    /// # Return:
    ///   是否命中并渲染
    pub(crate) fn render_if_ready(
        &self,
        key: &TerminalImageKey,
        render: impl FnOnce(&mut TerminalImage),
    ) -> bool {
        let mut inner = self.inner.borrow_mut();
        inner.tick = inner.tick.wrapping_add(1);
        let tick = inner.tick;
        let Some(slot) = inner
            .entries
            .get_mut(key.identity())
            .and_then(|slots| slots.iter_mut().find(|slot| slot.key == *key))
        else {
            return false;
        };
        slot.last_used = tick;
        render(&mut slot.image);
        true
    }

    /// 是否已缓存该终端图片键。该查询不更新 LRU。
    pub(crate) fn contains(&self, key: &TerminalImageKey) -> bool {
        matches!(
            self.inner.borrow().entries.get(key.identity()),
            Some(slots) if slots.iter().any(|slot| slot.key == *key)
        )
    }

    /// 对应终端成品已经可用。该查询不更新 LRU。
    pub(crate) fn ready(&self, key: &TerminalImageKey) -> bool {
        self.contains(key)
    }

    /// 装入一条编码好的终端图片：相同键替换现有项，全局越字节预算逐出最久未渲染槽。
    ///
    /// # Params:
    ///   - `key`: 源图片或 rasterized 成品身份
    ///   - `image`: 编码好的终端图片
    ///   - `bytes`: 该协议估算的常驻字节数(worker 侧算好带入)
    pub(crate) fn insert(&self, key: &TerminalImageKey, image: TerminalImage, bytes: u64) {
        let inner = &mut *self.inner.borrow_mut();
        inner.tick = inner.tick.wrapping_add(1);
        let last_used = inner.tick;
        let identity = key.identity().clone();
        let slots = inner.entries.entry(identity.clone()).or_default();
        if let Some(slot) = slots.iter_mut().find(|slot| slot.key == *key) {
            inner.total_bytes = inner.total_bytes.saturating_sub(slot.bytes);
            slot.image = image;
            slot.bytes = bytes;
            slot.last_used = last_used;
        } else {
            slots.push(Slot {
                key: key.clone(),
                image,
                bytes,
                last_used,
            });
        }
        inner.total_bytes = inner.total_bytes.saturating_add(bytes);
        Self::evict_over_budget(inner, self.budget, Some(key));
    }

    /// 移除某图片身份的全部尺寸槽。
    pub(crate) fn remove(&self, identity: &ImageIdentity) {
        let mut inner = self.inner.borrow_mut();
        if let Some(slots) = inner.entries.remove(identity) {
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

    /// 清空全部终端图片成品。
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
    fn evict_over_budget(inner: &mut Inner, budget: u64, keep: Option<&TerminalImageKey>) {
        while inner.total_bytes > budget {
            let victim = inner
                .entries
                .iter()
                .flat_map(|(identity, slots)| slots.iter().map(move |slot| (identity, slot)))
                .filter(|(_, slot)| keep != Some(&slot.key))
                .min_by_key(|(_, s)| s.last_used)
                .map(|(_, slot)| slot.key.clone());
            let Some(victim) = victim else {
                break;
            };
            Self::remove_slot(inner, &victim);
        }
    }

    /// 摘除一个终端图片槽并回收字节；该图片无剩余槽时删除映射。
    fn remove_slot(inner: &mut Inner, key: &TerminalImageKey) {
        let Some(slots) = inner.entries.get_mut(key.identity()) else {
            return;
        };
        if let Some(idx) = slots.iter().position(|slot| slot.key == *key) {
            let removed = slots.swap_remove(idx);
            inner.total_bytes = inner.total_bytes.saturating_sub(removed.bytes);
        }
        if slots.is_empty() {
            inner.entries.remove(key.identity());
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::MediaUrl;

    use super::TerminalImageCache;
    use crate::image::key::{ImageIdentity, PixelSize, TerminalImageKey};
    use crate::image::terminal::TerminalImage;

    /// 造一个 halfblocks 协议(不依赖真实终端探测)。字节由调用方另行指定,与协议无关。
    fn proto() -> TerminalImage {
        TerminalImage::test_halfblocks()
    }

    /// 造第 `n` 张封面 URL。
    fn url(n: u32) -> color_eyre::Result<MediaUrl> {
        Ok(MediaUrl::remote(&format!("https://example.com/{n}.jpg"))?)
    }

    /// 向终端图片缓存插入一项 rasterized 测试成品。
    fn insert(
        cache: &TerminalImageCache,
        url: &MediaUrl,
        dims: (u16, u16),
        protocol: TerminalImage,
        bytes: u64,
    ) {
        cache.insert(
            &TerminalImageKey::rasterized(
                ImageIdentity::Url(url.clone()),
                PixelSize::from_cells(dims, (1, 1)),
            ),
            protocol,
            bytes,
        );
    }

    /// 查询测试 URL 与尺寸是否已缓存。
    fn contains(cache: &TerminalImageCache, url: &MediaUrl, dims: (u16, u16)) -> bool {
        cache.contains(&TerminalImageKey::rasterized(
            ImageIdentity::Url(url.clone()),
            PixelSize::from_cells(dims, (1, 1)),
        ))
    }

    /// 渲染命中测试 URL 与尺寸。
    fn render(
        cache: &TerminalImageCache,
        url: &MediaUrl,
        dims: (u16, u16),
        render: impl FnOnce(&mut TerminalImage),
    ) -> bool {
        cache.render_if_ready(
            &TerminalImageKey::rasterized(
                ImageIdentity::Url(url.clone()),
                PixelSize::from_cells(dims, (1, 1)),
            ),
            render,
        )
    }

    /// 删除测试 URL 的全部终端成品。
    fn remove(cache: &TerminalImageCache, url: &MediaUrl) {
        cache.remove(&ImageIdentity::Url(url.clone()));
    }

    /// 未越预算:全部留驻。
    #[test]
    fn under_budget_keeps_all() -> color_eyre::Result<()> {
        let cache = TerminalImageCache::new(/*budget*/ 1_000);
        for n in 0..3 {
            insert(&cache, &url(n)?, (10, 10), proto(), /*bytes*/ 100);
        }
        assert!(contains(&cache, &url(0)?, (10, 10)));
        assert!(contains(&cache, &url(2)?, (10, 10)));
        Ok(())
    }

    /// 尺寸不一致按未命中：`contains` 为假，`render` 不触发闭包。
    #[test]
    fn dims_mismatch_is_miss() -> color_eyre::Result<()> {
        let cache = TerminalImageCache::new(/*budget*/ 1_000);
        let u0 = url(0)?;
        insert(&cache, &u0, (10, 10), proto(), /*bytes*/ 100);

        assert!(!contains(&cache, &u0, (20, 20)), "尺寸不同不算命中");
        let mut rendered = false;
        let hit = render(&cache, &u0, (20, 20), |_| rendered = true);
        assert!(!hit, "尺寸不一致时 render 应返回 false");
        assert!(!rendered, "未命中不应执行渲染闭包");
        Ok(())
    }

    /// 越预算：逐出最久未渲染者，`render` touch 过的受保护。
    #[test]
    fn evicts_least_recently_rendered() -> color_eyre::Result<()> {
        // 每条 100 字节,预算 300 恰容 3 条,第 4 条触发逐 1。
        let cache = TerminalImageCache::new(/*budget*/ 300);
        let (u0, u1, u2, u3) = (url(0)?, url(1)?, url(2)?, url(3)?);
        insert(&cache, &u0, (10, 10), proto(), 100);
        insert(&cache, &u1, (10, 10), proto(), 100);
        insert(&cache, &u2, (10, 10), proto(), 100);

        // 渲染 u0 → 变最近;此刻最久未渲染是 u1。
        assert!(render(&cache, &u0, (10, 10), |_| {}));

        insert(&cache, &u3, (10, 10), proto(), 100);

        assert!(!contains(&cache, &u1, (10, 10)), "u1 最久未渲染,被逐");
        assert!(contains(&cache, &u0, (10, 10)), "u0 被 render 保护");
        assert!(contains(&cache, &u3, (10, 10)), "刚插入的 u3 留驻");
        Ok(())
    }

    /// remove / clear 正确回收字节:清空后再插入不受旧账拖累。
    #[test]
    fn remove_and_clear_reclaim_bytes() -> color_eyre::Result<()> {
        let cache = TerminalImageCache::new(/*budget*/ 300);
        let (u0, u1, u2) = (url(0)?, url(1)?, url(2)?);
        insert(&cache, &u0, (10, 10), proto(), 300); // 占满
        remove(&cache, &u0);
        assert!(cache.is_empty(), "remove 后为空");

        // 账已清零,再塞满额三条不会因旧 300 立刻逐出。
        insert(&cache, &u1, (10, 10), proto(), 100);
        insert(&cache, &u2, (10, 10), proto(), 100);
        assert!(contains(&cache, &u1, (10, 10)));
        assert!(contains(&cache, &u2, (10, 10)));

        cache.clear();
        assert!(cache.is_empty(), "clear 后为空");
        Ok(())
    }

    /// 同一 URL 两个尺寸并存(常规面板 + 全屏):互不覆盖,各自命中渲染。
    #[test]
    fn same_url_two_dims_coexist() -> color_eyre::Result<()> {
        let cache = TerminalImageCache::new(/*budget*/ 1_000);
        let u = url(0)?;
        insert(&cache, &u, (10, 10), proto(), /*bytes*/ 100);
        insert(&cache, &u, (40, 20), proto(), /*bytes*/ 100);

        assert!(contains(&cache, &u, (10, 10)), "面板尺寸应保留");
        assert!(contains(&cache, &u, (40, 20)), "全屏尺寸应并存");
        assert!(render(&cache, &u, (10, 10), |_| {}), "面板尺寸可命中");
        assert!(render(&cache, &u, (40, 20), |_| {}), "全屏尺寸可命中");
        Ok(())
    }

    /// remove 清掉该 URL 全部尺寸并回收字节。
    #[test]
    fn remove_clears_all_dims() -> color_eyre::Result<()> {
        let cache = TerminalImageCache::new(/*budget*/ 200);
        let u = url(0)?;
        insert(&cache, &u, (10, 10), proto(), 100);
        insert(&cache, &u, (20, 20), proto(), 100);
        remove(&cache, &u);
        assert!(cache.is_empty(), "remove 后所有尺寸清空");

        // 字节账清零:预算 200 再装两条 100 不触发逐出。
        insert(&cache, &u, (10, 10), proto(), 100);
        insert(&cache, &u, (20, 20), proto(), 100);
        assert!(contains(&cache, &u, (10, 10)));
        assert!(contains(&cache, &u, (20, 20)));
        Ok(())
    }

    /// 同 (URL, 尺寸) 重复插入是替换:字节不重复记账。
    #[test]
    fn same_dims_reinsert_replaces_no_double_count() -> color_eyre::Result<()> {
        let cache = TerminalImageCache::new(/*budget*/ 250);
        let (u0, u1) = (url(0)?, url(1)?);
        insert(&cache, &u0, (10, 10), proto(), 100);
        insert(&cache, &u0, (10, 10), proto(), 100);
        // 替换后总账应为 100;再入 100 合计 200 ≤ 250,谁都不该被逐。
        insert(&cache, &u1, (10, 10), proto(), 100);
        assert!(
            contains(&cache, &u0, (10, 10)),
            "重复插入不应虚增字节导致逐出"
        );
        assert!(contains(&cache, &u1, (10, 10)));
        Ok(())
    }
}

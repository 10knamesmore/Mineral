//! 图片成品的字节预算 LRU，管理使用顺序、可见工作集与内容版本。
//!
//! 同一实现由独立实例分别承载低清 preview 与会话协议的稳态成品。Kitty 原样
//! 模式只按图片身份缓存；光栅成品按物理像素、字符网格与格边策略并存。
//! 行内缩略图保留独立的低清尺寸键。
//!
//! 每条字节由编码成品报告，缓存只记账不重算。最近准备的显示需求包含的工作集不被后台预热逐出。

use mineral_common::revision::Revision;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::image::key::{ImageIdentity, TerminalImageKey};

/// 某图片身份的一条终端成品槽。
struct Slot<T> {
    /// 源图片、行内缩略图或 rasterized 成品的缓存键。
    key: TerminalImageKey,

    /// 图片引擎自己的终端图片成品。
    image: T,

    /// 终端成品实际持有的像素或协议 payload 字节数，记账用。
    bytes: u64,

    /// 上次被准备入口登记的单调序号;最小者最久未使用,优先逐出。
    last_used: Revision,

    /// 成品内容写入版本，不随准备或协议提交改变。
    revision: Revision,
}

/// 终端图片成品的字节预算 LRU；最近准备的可见工作集允许暂时超出预算。
pub(crate) struct ImageCache<T> {
    /// 图片身份 → 源图片、行内缩略图或各 rasterized 尺寸的槽。
    entries: FxHashMap<ImageIdentity, Vec<Slot<T>>>,

    /// 当前占用字节合计(所有 [`Slot::bytes`] 之和)。
    total_bytes: u64,

    /// 缓存访问修订；写入时同时标记内容版本，后续访问只更新 `last_used`。
    tick: Revision,

    /// 最近准备的显示需求包含的成品，回填期间保留整个可见工作集。
    visible: FxHashSet<TerminalImageKey>,

    /// 本次准备请求过的键，包含尚在编码的成品。
    observed: FxHashSet<TerminalImageKey>,

    /// 字节预算上限，由所属 preview 或 terminal cache 配置提供。
    budget: u64,
}

impl<T> ImageCache<T> {
    /// 建空缓存,字节预算为 `budget`。
    ///
    /// # Params:
    ///   - `budget`: 终端图片成品的常驻字节上限
    pub(crate) fn new(budget: u64) -> Self {
        Self {
            entries: FxHashMap::default(),
            total_bytes: 0,
            tick: Revision::default(),
            visible: FxHashSet::default(),
            observed: FxHashSet::default(),
            budget,
        }
    }

    /// 只读借用已就绪成品，不登记使用或修改协议状态。
    pub(crate) fn get(&self, key: &TerminalImageKey) -> Option<&T> {
        self.slot(key).map(|slot| &slot.image)
    }

    /// 读取该采样成品的内容版本，不复制其他尺寸的键。
    pub(crate) fn revision(&self, key: &TerminalImageKey) -> Option<Revision> {
        self.slot(key).map(|slot| slot.revision)
    }

    fn slot(&self, key: &TerminalImageKey) -> Option<&Slot<T>> {
        self.entries
            .get(key.identity())?
            .iter()
            .find(|slot| slot.key == *key)
    }

    /// 准备阶段登记成品需求并更新使用顺序，绘制不参与缓存保活。
    pub(crate) fn observe(&mut self, key: &TerminalImageKey) {
        self.observed.insert(key.clone());
        self.tick = self.tick.next();
        if let Some(slot) = self
            .entries
            .get_mut(key.identity())
            .and_then(|slots| slots.iter_mut().find(|slot| slot.key == *key))
        {
            slot.last_used = self.tick;
        }
    }

    /// 借用成品更新其输出状态，不改变内容版本或缓存保活关系。
    pub(crate) fn get_mut(&mut self, key: &TerminalImageKey) -> Option<&mut T> {
        self.entries
            .get_mut(key.identity())?
            .iter_mut()
            .find(|slot| slot.key == *key)
            .map(|slot| &mut slot.image)
    }

    /// 是否已缓存该终端图片键。该查询不更新 LRU。
    pub(crate) fn contains(&self, key: &TerminalImageKey) -> bool {
        matches!(
            self.entries.get(key.identity()),
            Some(slots) if slots.iter().any(|slot| slot.key == *key)
        )
    }

    /// 在 worker 回填前更新可见工作集；图片离开屏幕后回收超预算成品。
    pub(crate) fn advance_frame(&mut self) {
        let observed = std::mem::take(&mut self.observed);
        let released = self.visible.iter().any(|key| !observed.contains(key));
        self.visible = observed;
        if released {
            self.evict_over_budget(/*keep*/ None);
        }
    }

    /// 装入一条编码好的终端图片：相同键替换现有项，全局越字节预算逐出最久未渲染槽。
    ///
    /// # Params:
    ///   - `key`: 源图片或 rasterized 成品身份
    ///   - `image`: 编码好的终端图片
    ///   - `bytes`: 该成品持有的像素或协议 payload 字节数
    pub(crate) fn insert(&mut self, key: &TerminalImageKey, image: T, bytes: u64) {
        self.tick = self.tick.next();
        let last_used = self.tick;
        let slots = self.entries.entry(key.identity().clone()).or_default();
        if let Some(slot) = slots.iter_mut().find(|slot| slot.key == *key) {
            self.total_bytes = self.total_bytes.saturating_sub(slot.bytes);
            slot.image = image;
            slot.bytes = bytes;
            slot.last_used = last_used;
            slot.revision = last_used;
        } else {
            slots.push(Slot {
                key: key.clone(),
                image,
                bytes,
                last_used,
                revision: last_used,
            });
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.evict_over_budget(Some(key));
    }

    /// 仅观察一个图片身份的就绪尺寸与内容版本，不续命也不保留像素。
    pub(crate) fn revisions(&self, identity: &ImageIdentity) -> Vec<(TerminalImageKey, Revision)> {
        self.entries.get(identity).map_or_else(Vec::new, |slots| {
            slots
                .iter()
                .map(|slot| (slot.key.clone(), slot.revision))
                .collect()
        })
    }

    /// 移除某图片身份的全部尺寸槽。
    pub(crate) fn remove(&mut self, identity: &ImageIdentity) {
        if let Some(slots) = self.entries.remove(identity) {
            let freed = slots
                .iter()
                .fold(0u64, |acc, s| acc.saturating_add(s.bytes));
            self.total_bytes = self.total_bytes.saturating_sub(freed);
        }
    }

    /// 原图逐出或重新解码时移除高清成品，独立的行内缩略图继续受协议预算管理。
    pub(crate) fn remove_decoded(&mut self, identity: &ImageIdentity) {
        let Some(slots) = self.entries.get_mut(identity) else {
            return;
        };
        let mut freed = 0_u64;
        slots.retain(|slot| {
            if matches!(slot.key, TerminalImageKey::Thumbnail { .. }) {
                true
            } else {
                freed = freed.saturating_add(slot.bytes);
                false
            }
        });
        if slots.is_empty() {
            self.entries.remove(identity);
        }
        self.total_bytes = self.total_bytes.saturating_sub(freed);
    }

    /// 热更新字节预算，立即回收超预算的未显示成品，保留可见工作集。
    ///
    /// # Params:
    ///   - `budget`: 新预算(字节)
    pub(crate) fn set_budget(&mut self, budget: u64) {
        self.budget = budget;
        self.evict_over_budget(/*keep*/ None);
    }

    /// 是否为空(测试 / 断言用)。
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 逐出最久未渲染槽直到回落预算内。`keep` 是刚插入槽,永不逐出。
    fn evict_over_budget(&mut self, keep: Option<&TerminalImageKey>) {
        while self.total_bytes > self.budget {
            let victim = self
                .entries
                .values()
                .flatten()
                .filter(|slot| keep != Some(&slot.key))
                .filter(|slot| !self.visible.contains(&slot.key))
                .min_by_key(|slot| slot.last_used)
                .map(|slot| slot.key.clone());
            let Some(victim) = victim else {
                break;
            };
            mineral_log::debug!(target: "cover_cache", key = ?victim,
                cached_bytes = self.total_bytes, budget_bytes = self.budget,
                visible_images = self.visible.len(), "evict terminal image outside visible working set");
            self.remove_key(&victim);
        }
    }

    /// 摘除一个终端图片槽并回收字节；该图片无剩余槽时删除映射。
    pub(crate) fn remove_key(&mut self, key: &TerminalImageKey) {
        let Some(slots) = self.entries.get_mut(key.identity()) else {
            return;
        };
        if let Some(idx) = slots.iter().position(|slot| slot.key == *key) {
            let removed = slots.swap_remove(idx);
            self.total_bytes = self.total_bytes.saturating_sub(removed.bytes);
        }
        if slots.is_empty() {
            self.entries.remove(key.identity());
        }
    }
}

#[cfg(test)]
mod tests {
    use mineral_model::MediaUrl;

    use super::ImageCache;
    use crate::image::key::{ImageIdentity, PixelSize, TerminalImageKey};
    use crate::image::prepared_image::PreparedImage;

    /// 造一个 halfblocks 协议(不依赖真实终端探测)。字节由调用方另行指定,与协议无关。
    fn proto() -> PreparedImage {
        PreparedImage::test_halfblock()
    }

    /// 造第 `n` 张封面 URL。
    fn url(n: u32) -> color_eyre::Result<MediaUrl> {
        Ok(MediaUrl::remote(&format!("https://example.com/{n}.jpg"))?)
    }

    /// 向终端图片缓存插入一项 rasterized 测试成品。
    fn insert(
        cache: &mut ImageCache<PreparedImage>,
        url: &MediaUrl,
        dims: (u16, u16),
        protocol: PreparedImage,
        bytes: u64,
    ) {
        cache.insert(
            &TerminalImageKey::rasterized(
                ImageIdentity::Url(url.clone()),
                PixelSize::from_cells(dims, (1, 1)),
                dims,
                crate::config::CoverCellFit::Contain,
            ),
            protocol,
            bytes,
        );
    }

    /// 查询测试 URL 与尺寸是否已缓存。
    fn contains(cache: &ImageCache<PreparedImage>, url: &MediaUrl, dims: (u16, u16)) -> bool {
        cache.contains(&TerminalImageKey::rasterized(
            ImageIdentity::Url(url.clone()),
            PixelSize::from_cells(dims, (1, 1)),
            dims,
            crate::config::CoverCellFit::Contain,
        ))
    }

    /// 渲染命中测试 URL 与尺寸。
    fn render(
        cache: &mut ImageCache<PreparedImage>,
        url: &MediaUrl,
        dims: (u16, u16),
        render: impl FnOnce(&PreparedImage),
    ) -> bool {
        let key = TerminalImageKey::rasterized(
            ImageIdentity::Url(url.clone()),
            PixelSize::from_cells(dims, (1, 1)),
            dims,
            crate::config::CoverCellFit::Contain,
        );
        cache.observe(&key);
        if let Some(image) = cache.get(&key) {
            render(image);
            true
        } else {
            false
        }
    }

    /// 删除测试 URL 的全部终端成品。
    fn remove(cache: &mut ImageCache<PreparedImage>, url: &MediaUrl) {
        cache.remove(&ImageIdentity::Url(url.clone()));
    }

    /// 未越预算:全部留驻。
    #[test]
    fn under_budget_keeps_all() -> color_eyre::Result<()> {
        let mut cache = ImageCache::new(/*budget*/ 1_000);
        for n in 0..3 {
            insert(&mut cache, &url(n)?, (10, 10), proto(), /*bytes*/ 100);
        }
        assert!(contains(&cache, &url(0)?, (10, 10)));
        assert!(contains(&cache, &url(2)?, (10, 10)));
        Ok(())
    }

    /// 尺寸不一致按未命中：`contains` 为假，`render` 不触发闭包。
    #[test]
    fn dims_mismatch_is_miss() -> color_eyre::Result<()> {
        let mut cache = ImageCache::new(/*budget*/ 1_000);
        let u0 = url(0)?;
        insert(&mut cache, &u0, (10, 10), proto(), /*bytes*/ 100);

        assert!(!contains(&cache, &u0, (20, 20)), "尺寸不同不算命中");
        let mut rendered = false;
        let hit = render(&mut cache, &u0, (20, 20), |_| rendered = true);
        assert!(!hit, "尺寸不一致时 render 应返回 false");
        assert!(!rendered, "未命中不应执行渲染闭包");
        Ok(())
    }

    /// 越预算：逐出最久未渲染者，`render` touch 过的受保护。
    #[test]
    fn evicts_least_recently_rendered() -> color_eyre::Result<()> {
        // 每条 100 字节,预算 300 恰容 3 条,第 4 条触发逐 1。
        let mut cache = ImageCache::new(/*budget*/ 300);
        let (u0, u1, u2, u3) = (url(0)?, url(1)?, url(2)?, url(3)?);
        insert(&mut cache, &u0, (10, 10), proto(), 100);
        insert(&mut cache, &u1, (10, 10), proto(), 100);
        insert(&mut cache, &u2, (10, 10), proto(), 100);

        // 渲染 u0 → 变最近;此刻最久未渲染是 u1。
        assert!(render(&mut cache, &u0, (10, 10), |_| {}));

        insert(&mut cache, &u3, (10, 10), proto(), 100);

        assert!(!contains(&cache, &u1, (10, 10)), "u1 最久未渲染,被逐");
        assert!(contains(&cache, &u0, (10, 10)), "u0 被 render 保护");
        assert!(contains(&cache, &u3, (10, 10)), "刚插入的 u3 留驻");
        Ok(())
    }

    /// 同屏图片总量超过预算时保持稳定，离屏后重新受预算约束。
    #[test]
    fn visible_images_survive_budget_pressure_until_hidden() -> color_eyre::Result<()> {
        let mut cache = ImageCache::new(/*budget*/ 150);
        let (first, second, warm) = (url(0)?, url(1)?, url(2)?);
        // 两个显示位置即便尚未编码，也应进入下一拍回填时的可见工作集。
        assert!(!render(&mut cache, &first, (10, 10), |_| {}));
        assert!(!render(&mut cache, &second, (10, 10), |_| {}));
        cache.advance_frame();
        insert(&mut cache, &first, (10, 10), proto(), /*bytes*/ 100);
        insert(&mut cache, &second, (10, 10), proto(), /*bytes*/ 100);
        for _ in 0..3 {
            insert(&mut cache, &warm, (10, 10), proto(), /*bytes*/ 25);
            assert!(
                render(&mut cache, &first, (10, 10), |_| {}),
                "第一张不能退回 halfblock"
            );
            assert!(
                render(&mut cache, &second, (10, 10), |_| {}),
                "第二张不能退回 halfblock"
            );
            cache.advance_frame();
        }
        assert!(render(&mut cache, &first, (10, 10), |_| {}));
        cache.advance_frame();
        assert!(contains(&cache, &first, (10, 10)));
        assert!(!contains(&cache, &second, (10, 10)), "离屏后应恢复预算约束");
        assert!(cache.total_bytes <= cache.budget);
        Ok(())
    }

    /// remove 正确回收字节，再插入不受旧账拖累。
    #[test]
    fn remove_reclaims_bytes() -> color_eyre::Result<()> {
        let mut cache = ImageCache::new(/*budget*/ 300);
        let (u0, u1, u2) = (url(0)?, url(1)?, url(2)?);
        insert(&mut cache, &u0, (10, 10), proto(), 300); // 占满
        remove(&mut cache, &u0);
        assert!(cache.is_empty(), "remove 后为空");

        // 账已清零,再塞满额三条不会因旧 300 立刻逐出。
        insert(&mut cache, &u1, (10, 10), proto(), 100);
        insert(&mut cache, &u2, (10, 10), proto(), 100);
        assert!(contains(&cache, &u1, (10, 10)));
        assert!(contains(&cache, &u2, (10, 10)));

        Ok(())
    }

    /// 同一 URL 两个尺寸并存(常规面板 + 全屏):互不覆盖,各自命中渲染。
    #[test]
    fn same_url_two_dims_coexist() -> color_eyre::Result<()> {
        let mut cache = ImageCache::new(/*budget*/ 1_000);
        let u = url(0)?;
        insert(&mut cache, &u, (10, 10), proto(), /*bytes*/ 100);
        insert(&mut cache, &u, (40, 20), proto(), /*bytes*/ 100);

        assert!(contains(&cache, &u, (10, 10)), "面板尺寸应保留");
        assert!(contains(&cache, &u, (40, 20)), "全屏尺寸应并存");
        assert!(render(&mut cache, &u, (10, 10), |_| {}), "面板尺寸可命中");
        assert!(render(&mut cache, &u, (40, 20), |_| {}), "全屏尺寸可命中");
        Ok(())
    }

    /// remove 清掉该 URL 全部尺寸并回收字节。
    #[test]
    fn remove_clears_all_dims() -> color_eyre::Result<()> {
        let mut cache = ImageCache::new(/*budget*/ 200);
        let u = url(0)?;
        insert(&mut cache, &u, (10, 10), proto(), 100);
        insert(&mut cache, &u, (20, 20), proto(), 100);
        remove(&mut cache, &u);
        assert!(cache.is_empty(), "remove 后所有尺寸清空");

        // 字节账清零:预算 200 再装两条 100 不触发逐出。
        insert(&mut cache, &u, (10, 10), proto(), 100);
        insert(&mut cache, &u, (20, 20), proto(), 100);
        assert!(contains(&cache, &u, (10, 10)));
        assert!(contains(&cache, &u, (20, 20)));
        Ok(())
    }

    /// 同 (URL, 尺寸) 重复插入是替换:字节不重复记账。
    #[test]
    fn same_dims_reinsert_replaces_no_double_count() -> color_eyre::Result<()> {
        let mut cache = ImageCache::new(/*budget*/ 250);
        let (u0, u1) = (url(0)?, url(1)?);
        insert(&mut cache, &u0, (10, 10), proto(), 100);
        insert(&mut cache, &u0, (10, 10), proto(), 100);
        // 替换后总账应为 100;再入 100 合计 200 ≤ 250,谁都不该被逐。
        insert(&mut cache, &u1, (10, 10), proto(), 100);
        assert!(
            contains(&cache, &u0, (10, 10)),
            "重复插入不应虚增字节导致逐出"
        );
        assert!(contains(&cache, &u1, (10, 10)));
        Ok(())
    }
}

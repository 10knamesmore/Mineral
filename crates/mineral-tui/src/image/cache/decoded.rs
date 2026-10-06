//! 封面像素的字节预算 LRU 缓存。
//!
//! 准备阶段显式登记可见工作集与使用顺序；只读查询不保活。
//! 可见工作集可以超出预算，离屏后重新参与回收。

use std::sync::Arc;

use image::DynamicImage;
use mineral_model::MediaUrl;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::image::CoverFingerprint;

/// 一条缓存项。
struct Entry {
    /// 按配置尺寸解码的封面像素(像素缓冲是常驻内存大头)。
    image: Arc<DynamicImage>,

    /// 内容指纹：同一张图的不同 URL(Netease 尺寸变体)靠它相认。
    fingerprint: CoverFingerprint,

    /// 该图像素字节数,记账用,免逐出时重算。
    bytes: u64,

    /// 上次被准备入口登记的单调序号;最小者最久未用,优先逐出。
    last_used: u64,

    /// 这份像素写入时的序号，不随可见性记账改变。
    revision: u64,
}

/// 封面像素缓存:字节预算 LRU。
///
/// `observe_visible` 更新 LRU 并登记可见工作集。回填超预算时只逐出未显示的图片，
/// 返回其 URL 供调用方清理派生的协议和色板；大图不会因后台预热而反复重解码。
pub(crate) struct CoverCache {
    /// URL → 缓存项。
    entries: FxHashMap<MediaUrl, Entry>,

    /// 当前占用字节合计(所有 `Entry::bytes` 之和)。
    total_bytes: u64,

    /// 单调访问计数器,每次 `observe_visible` / `insert` 取一个新值赋给 `last_used`。
    tick: u64,

    /// 字节预算上限(来自配置 `cover.cache.image`)。
    budget: u64,

    /// 最近准备的显示需求包含的图片，后台解码回填不能逐出它们。
    visible: FxHashSet<MediaUrl>,

    /// 本次准备所需的图片，包含尚未解码的 URL。
    observed: FxHashSet<MediaUrl>,
}

impl CoverCache {
    /// 建空缓存,字节预算为 `budget`。
    ///
    /// # Params:
    ///   - `budget`: 常驻像素的字节上限;`insert` 越过即逐出最久未用项
    pub(crate) fn new(budget: u64) -> Self {
        Self {
            entries: FxHashMap::default(),
            total_bytes: 0,
            tick: 0,
            budget,
            visible: FxHashSet::default(),
            observed: FxHashSet::default(),
        }
    }

    /// 只读取已解码像素；使用记录由准备阶段更新。
    pub(crate) fn get(&self, url: &MediaUrl) -> Option<&Arc<DynamicImage>> {
        let entry = self.entries.get(url)?;
        Some(&entry.image)
    }

    /// 已解码内容的版本；逐出后缺失，重新装入会得到新版本。
    pub(crate) fn revision(&self, url: &MediaUrl) -> Option<u64> {
        self.entries.get(url).map(|entry| entry.revision)
    }

    /// 是否已缓存该 URL。**不**更新 LRU 顺序(探测用,非显示,不该借此续命)。
    pub(crate) fn contains_key(&self, url: &MediaUrl) -> bool {
        self.entries.contains_key(url)
    }

    /// 测试用插入:指纹就地从图里算(测试只关心账目与逐出)。
    #[cfg(test)]
    pub(crate) fn insert_test(
        &mut self,
        url: &MediaUrl,
        image: Arc<DynamicImage>,
    ) -> Vec<MediaUrl> {
        let fingerprint = CoverFingerprint::of(&image);
        self.insert(url, image, fingerprint)
    }

    /// 两张已解码封面是否同一张图(内容指纹比对)。**不**更新 LRU 顺序。
    ///
    /// # Params:
    ///   - `left` / `right`: 待比较的两个封面 URL;任一未解码时为 `false`
    pub(crate) fn same_picture(&self, left: &MediaUrl, right: &MediaUrl) -> bool {
        let (Some(left), Some(right)) = (self.entries.get(left), self.entries.get(right)) else {
            return false;
        };
        left.fingerprint.matches(&right.fingerprint)
    }

    /// 登记当前帧实际显示的图片；预取和预编码不调用此入口。
    pub(crate) fn observe_visible(&mut self, url: &MediaUrl) {
        self.observed.insert(url.clone());
        let tick = self.next_tick();
        if let Some(entry) = self.entries.get_mut(url) {
            entry.last_used = tick;
        }
    }

    /// 在解码回填前更新可见工作集；离屏图片重新受预算约束。
    pub(crate) fn advance_frame(&mut self) -> Vec<MediaUrl> {
        let observed = std::mem::take(&mut self.observed);
        let released = self.visible.iter().any(|url| !observed.contains(url));
        self.visible = observed;
        if released {
            self.evict_over_budget(/*keep*/ None)
        } else {
            Vec::new()
        }
    }

    /// 当前缓存条数。
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// 插入 / 覆盖一张图,越预算则逐出最久未用项。
    ///
    /// # Params:
    ///   - `url`: 封面 URL(内部 clone 一份作 key)
    ///   - `image`: 解码后的图片
    ///   - `fingerprint`: 该图的内容指纹(解码 worker 里算好)
    ///
    /// # Return:
    ///   被逐出的 URL 列表(不含刚插入的 `url`);未触发逐出时为空。
    pub(crate) fn insert(
        &mut self,
        url: &MediaUrl,
        image: Arc<DynamicImage>,
        fingerprint: CoverFingerprint,
    ) -> Vec<MediaUrl> {
        let bytes = image_bytes(&image);
        let last_used = self.next_tick();
        if let Some(old) = self.entries.insert(
            url.clone(),
            Entry {
                image,
                fingerprint,
                bytes,
                last_used,
                revision: last_used,
            },
        ) {
            self.total_bytes = self.total_bytes.saturating_sub(old.bytes);
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.evict_over_budget(Some(url))
    }

    /// 热更新字节预算，立即回收超预算的未显示图片，保留可见工作集。
    ///
    /// # Params:
    ///   - `budget`: 新预算(字节)
    ///
    /// # Return:
    ///   被逐出的 URL 列表(派生物联动清理用);未触发逐出时为空。
    pub(crate) fn set_budget(&mut self, budget: u64) -> Vec<MediaUrl> {
        self.budget = budget;
        self.evict_over_budget(/*keep*/ None)
    }

    /// 取下一个访问序号(单调递增,`wrapping` 免溢出 panic —— u64 实际到不了上限)。
    fn next_tick(&mut self) -> u64 {
        let next = self.tick.wrapping_add(1);
        self.tick = next;
        next
    }

    /// 逐出最久未用项直到回落预算内。`keep` 是刚插入项,永不逐出 —— 防单张即超预算时
    /// 把自己也逐掉(此时它超额留驻,靠下次 `insert` 引入更小工作集时自然回落)。
    fn evict_over_budget(&mut self, keep: Option<&MediaUrl>) -> Vec<MediaUrl> {
        let mut evicted = Vec::<MediaUrl>::new();
        while self.total_bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(url, _)| Some(*url) != keep && !self.visible.contains(*url))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(url, _)| url.clone());
            let Some(victim) = victim else {
                break;
            };
            mineral_log::debug!(target: "cover_cache", url = %victim,
                cached_bytes = self.total_bytes, budget_bytes = self.budget,
                visible_images = self.visible.len(), "evict decoded cover outside visible working set");
            if let Some(entry) = self.entries.remove(&victim) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
            evicted.push(victim);
        }
        evicted
    }
}

/// 估算一张图的常驻字节数(解码后像素缓冲长度)。
fn image_bytes(image: &DynamicImage) -> u64 {
    u64::try_from(image.as_bytes().len()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use image::{DynamicImage, RgbImage};
    use mineral_model::MediaUrl;

    use super::CoverCache;

    /// 造一张 `side × side` 的 RGB 图,常驻字节 = `side * side * 3`。
    fn img(side: u32) -> Arc<DynamicImage> {
        Arc::new(DynamicImage::ImageRgb8(RgbImage::new(side, side)))
    }

    /// 造第 `n` 张的远端封面 URL。
    fn url(n: u32) -> color_eyre::Result<MediaUrl> {
        Ok(MediaUrl::remote(&format!("https://example.com/{n}.jpg"))?)
    }

    /// 未越预算:全部留驻,无逐出。
    #[test]
    fn under_budget_keeps_all() -> color_eyre::Result<()> {
        let mut cache = CoverCache::new(/*budget*/ 1_000_000);
        for n in 0..3 {
            let evicted = cache.insert_test(&url(n)?, img(100));
            assert!(evicted.is_empty(), "第 {n} 张未越预算不应逐出");
        }
        assert_eq!(cache.len(), 3);
        assert!(cache.contains_key(&url(0)?));
        Ok(())
    }

    /// 越预算:逐出**最久未用**的一张,`get` touch 过的受保护。
    #[test]
    fn evicts_least_recently_used() -> color_eyre::Result<()> {
        // 单张 30_000 字节;预算 100_000 恰容 3 张,第 4 张触发逐 1。
        let mut cache = CoverCache::new(/*budget*/ 100_000);
        let (u0, u1, u2, u3) = (url(0)?, url(1)?, url(2)?, url(3)?);
        cache.insert_test(&u0, img(100));
        cache.insert_test(&u1, img(100));
        cache.insert_test(&u2, img(100));

        // 准备 u0 → 变最近;此刻最久未用是 u1。
        cache.observe_visible(&u0);
        assert!(cache.get(&u0).is_some());

        let evicted = cache.insert_test(&u3, img(100));

        assert_eq!(evicted, vec![u1.clone()], "应逐出最久未用的 u1");
        assert!(!cache.contains_key(&u1), "u1 已被逐");
        assert!(cache.contains_key(&u0), "u0 被 get 保护,留驻");
        assert!(cache.contains_key(&u3), "刚插入的 u3 留驻");
        assert_eq!(cache.len(), 3);
        Ok(())
    }

    /// 不 touch 的对照:纯按插入先后,逐出最早插入者。
    #[test]
    fn without_touch_evicts_oldest_inserted() -> color_eyre::Result<()> {
        let mut cache = CoverCache::new(/*budget*/ 100_000);
        let (u0, u1, u2, u3) = (url(0)?, url(1)?, url(2)?, url(3)?);
        cache.insert_test(&u0, img(100));
        cache.insert_test(&u1, img(100));
        cache.insert_test(&u2, img(100));

        let evicted = cache.insert_test(&u3, img(100));

        assert_eq!(evicted, vec![u0.clone()], "无 touch 时逐出最早插入的 u0");
        assert!(!cache.contains_key(&u0));
        Ok(())
    }

    /// 可见原图不能被后台解码逐出，否则终端成品也会被联动清理。
    #[test]
    fn visible_covers_survive_background_decode_until_hidden() -> color_eyre::Result<()> {
        let mut cache = CoverCache::new(/*budget*/ 50_000);
        let (first, second, warm) = (url(0)?, url(1)?, url(2)?);
        cache.observe_visible(&first);
        cache.observe_visible(&second);
        assert!(cache.advance_frame().is_empty());
        assert!(cache.insert_test(&first, img(/*side*/ 100)).is_empty());
        assert!(cache.insert_test(&second, img(/*side*/ 100)).is_empty());
        for _ in 0..3 {
            assert!(cache.insert_test(&warm, img(/*side*/ 50)).is_empty());
            assert!(cache.get(&first).is_some());
            assert!(cache.get(&second).is_some());
            cache.observe_visible(&first);
            cache.observe_visible(&second);
            assert!(cache.advance_frame().is_empty());
        }
        cache.observe_visible(&first);
        let evicted = cache.advance_frame();
        assert!(evicted.contains(&second), "离屏大图必须可被回收");
        assert!(cache.get(&first).is_some());
        assert!(!cache.contains_key(&second));
        assert!(cache.total_bytes <= cache.budget);
        Ok(())
    }

    /// 单张即超预算:把其余全逐光后仍超,该张超额留驻(不自逐),`len` 归 1。
    #[test]
    fn oversized_single_stays_after_evicting_rest() -> color_eyre::Result<()> {
        let mut cache = CoverCache::new(/*budget*/ 100_000);
        cache.insert_test(&url(0)?, img(100)); // 30_000
        cache.insert_test(&url(1)?, img(100)); // 30_000
        cache.insert_test(&url(2)?, img(100)); // 30_000

        // 一张 200×200 = 120_000 字节,单张即超 100_000 预算。
        let big = url(9)?;
        let evicted = cache.insert_test(&big, img(200));

        assert_eq!(evicted.len(), 3, "三张小图全被逐");
        assert!(cache.contains_key(&big), "超额大图仍留驻,不自逐");
        assert_eq!(cache.len(), 1);
        Ok(())
    }
}

//! 封面原图的字节预算 LRU 缓存。
//!
//! 渲染路径只有 `&AppState`,故 `get` 用 `&self` + 内部 `Cell` 记 LRU 顺序
//! (`Cell::set` 对 `Copy` 类型无运行时借用检查,无 panic 面);逐出只发生在
//! `&mut self` 的 `insert`。按字节而非条数封顶:封面尺寸不一,唯有字节预算能把
//! 常驻内存钉死在一个数上,与图大小解耦。

use std::cell::Cell;
use std::sync::Arc;

use image::DynamicImage;
use mineral_model::MediaUrl;
use rustc_hash::FxHashMap;

/// 一条缓存项。
struct Entry {
    /// 解码后的封面原图(像素缓冲是常驻内存大头)。
    image: Arc<DynamicImage>,

    /// 该图像素字节数,记账用,免逐出时重算。
    bytes: u64,

    /// 上次被 `get` 命中的单调序号;最小者最久未用,优先逐出。
    last_used: Cell<u64>,
}

/// 封面原图缓存:字节预算 LRU。
///
/// `get` 命中即把该项标为最近使用(经内部 `Cell`),从而保护正在显示/在播的封面
/// 不被逐出;`insert` 后若越预算,逐出最久未用项并返回其 URL 列表,供上层联动清理
/// 由该图派生的协议 / 色板缓存,避免"协议在但原图没了"的裂缝。
pub(crate) struct CoverCache {
    /// URL → 缓存项。
    entries: FxHashMap<MediaUrl, Entry>,

    /// 当前占用字节合计(所有 `Entry::bytes` 之和)。
    total_bytes: u64,

    /// 单调访问计数器,每次 `get` / `insert` 取一个新值赋给 `last_used`。
    tick: Cell<u64>,

    /// 字节预算上限(来自配置 `tui.cover.cache.image`)。
    budget: u64,
}

impl CoverCache {
    /// 建空缓存,字节预算为 `budget`。
    ///
    /// # Params:
    ///   - `budget`: 常驻原图的字节上限;`insert` 越过即逐出最久未用项
    pub(crate) fn new(budget: u64) -> Self {
        Self {
            entries: FxHashMap::default(),
            total_bytes: 0,
            tick: Cell::new(0),
            budget,
        }
    }

    /// 取图并标为最近使用(保护其不被后续 `insert` 逐出)。未命中返回 `None`。
    pub(crate) fn get(&self, url: &MediaUrl) -> Option<&Arc<DynamicImage>> {
        let entry = self.entries.get(url)?;
        entry.last_used.set(self.next_tick());
        Some(&entry.image)
    }

    /// 是否已缓存该 URL。**不**更新 LRU 顺序(探测用,非显示,不该借此续命)。
    pub(crate) fn contains_key(&self, url: &MediaUrl) -> bool {
        self.entries.contains_key(url)
    }

    /// 当前缓存条数。
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// 插入 / 覆盖一张图,越预算则逐出最久未用项。
    ///
    /// # Params:
    ///   - `url`: 封面 URL(内部 clone 一份作 key)
    ///   - `image`: 解码后的原图
    ///
    /// # Return:
    ///   被逐出的 URL 列表(不含刚插入的 `url`);未触发逐出时为空。
    pub(crate) fn insert(&mut self, url: &MediaUrl, image: Arc<DynamicImage>) -> Vec<MediaUrl> {
        let bytes = image_bytes(&image);
        let last_used = Cell::new(self.next_tick());
        if let Some(old) = self.entries.insert(
            url.clone(),
            Entry {
                image,
                bytes,
                last_used,
            },
        ) {
            self.total_bytes = self.total_bytes.saturating_sub(old.bytes);
        }
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.evict_over_budget(url)
    }

    /// 现调字节预算(配置热更):缩小立即逐出最久未用项直到回落,**不清整表**;
    /// 调大只放宽上限。
    ///
    /// # Params:
    ///   - `budget`: 新预算(字节)
    ///
    /// # Return:
    ///   被逐出的 URL 列表(派生物联动清理用);未触发逐出时为空。
    pub(crate) fn set_budget(&mut self, budget: u64) -> Vec<MediaUrl> {
        self.budget = budget;
        let mut evicted = Vec::<MediaUrl>::new();
        while self.total_bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.last_used.get())
                .map(|(url, _)| url.clone());
            let Some(victim) = victim else {
                break;
            };
            if let Some(entry) = self.entries.remove(&victim) {
                self.total_bytes = self.total_bytes.saturating_sub(entry.bytes);
            }
            evicted.push(victim);
        }
        evicted
    }

    /// 取下一个访问序号(单调递增,`wrapping` 免溢出 panic —— u64 实际到不了上限)。
    fn next_tick(&self) -> u64 {
        let next = self.tick.get().wrapping_add(1);
        self.tick.set(next);
        next
    }

    /// 逐出最久未用项直到回落预算内。`keep` 是刚插入项,永不逐出 —— 防单张即超预算时
    /// 把自己也逐掉(此时它超额留驻,靠下次 `insert` 引入更小工作集时自然回落)。
    fn evict_over_budget(&mut self, keep: &MediaUrl) -> Vec<MediaUrl> {
        let mut evicted = Vec::<MediaUrl>::new();
        while self.total_bytes > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(url, _)| *url != keep)
                .min_by_key(|(_, entry)| entry.last_used.get())
                .map(|(url, _)| url.clone());
            let Some(victim) = victim else {
                break;
            };
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
            let evicted = cache.insert(&url(n)?, img(100));
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
        cache.insert(&u0, img(100));
        cache.insert(&u1, img(100));
        cache.insert(&u2, img(100));

        // touch u0 → 变最近;此刻最久未用是 u1。
        assert!(cache.get(&u0).is_some());

        let evicted = cache.insert(&u3, img(100));

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
        cache.insert(&u0, img(100));
        cache.insert(&u1, img(100));
        cache.insert(&u2, img(100));

        let evicted = cache.insert(&u3, img(100));

        assert_eq!(evicted, vec![u0.clone()], "无 touch 时逐出最早插入的 u0");
        assert!(!cache.contains_key(&u0));
        Ok(())
    }

    /// 单张即超预算:把其余全逐光后仍超,该张超额留驻(不自逐),`len` 归 1。
    #[test]
    fn oversized_single_stays_after_evicting_rest() -> color_eyre::Result<()> {
        let mut cache = CoverCache::new(/*budget*/ 100_000);
        cache.insert(&url(0)?, img(100)); // 30_000
        cache.insert(&url(1)?, img(100)); // 30_000
        cache.insert(&url(2)?, img(100)); // 30_000

        // 一张 200×200 = 120_000 字节,单张即超 100_000 预算。
        let big = url(9)?;
        let evicted = cache.insert(&big, img(200));

        assert_eq!(evicted.len(), 3, "三张小图全被逐");
        assert!(cache.contains_key(&big), "超额大图仍留驻,不自逐");
        assert_eq!(cache.len(), 1);
        Ok(())
    }
}

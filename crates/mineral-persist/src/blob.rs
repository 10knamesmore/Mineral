//! 内容寻址文件缓存：内存索引 + sidecar(index.bin) + 启动对账 + LRU 驱逐。
//!
//! 落盘是带真实扩展名的原文件(`<subdir>/<hash>.<ext>`),可被图片/音频查看器直接打开;
//! `subdir` 用于按来源/类型分区同一缓存根。
//!
//! 缓存只是优化、永不是正确性依赖：命中是 best-effort，任何不确定(文件缺失/损坏/漂移)
//! 一律当 miss(`get` 返回 `None`)，由上层回退远端。

use std::path::{Path, PathBuf};

use color_eyre::eyre::WrapErr;
use mineral_log::{debug, info, trace};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// 单条缓存项的元数据。
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    /// 相对缓存根的文件路径 `<subdir>/<hash>.<ext>`。
    file: String,

    /// 字节数。
    bytes: u64,

    /// 最近访问的逻辑时钟值(用于 LRU 排序)。
    last_access: u64,
}

/// 索引(可序列化进 index.bin)。
#[derive(Default, Serialize, Deserialize)]
struct Index {
    /// key → Entry。
    entries: rustc_hash::FxHashMap<String, Entry>,

    /// 当前总字节。
    total_bytes: u64,

    /// 单调逻辑时钟(每次访问 +1，作 last_access)。
    clock: u64,
}

/// 内容寻址 blob 缓存。线程安全(内部 `Mutex`)，不依赖 tokio runtime。
pub struct BlobCache {
    /// 缓存目录。
    dir: PathBuf,

    /// 容量上限(字节)。
    capacity: u64,

    /// 内存索引。
    index: Mutex<Index>,
}

impl BlobCache {
    /// 打开(或创建)一个缓存目录，读 index.bin 并对账。
    ///
    /// # Params:
    ///   - `dir`: 缓存目录(不存在则创建)
    ///   - `capacity`: 容量上限字节
    ///
    /// # Return:
    ///   就绪的缓存；index.bin 不存在 / 损坏时视为空索引(只丢索引，不删文件)。
    pub fn open(dir: &Path, capacity: u64) -> color_eyre::Result<Self> {
        std::fs::create_dir_all(dir)
            .wrap_err_with(|| format!("创建 blob 缓存目录失败 dir={}", dir.display()))?;
        let mut index = Self::load_index(dir).unwrap_or_default();
        let before = index.entries.len();
        Self::reconcile(dir, &mut index);
        let after = index.entries.len();
        if after < before {
            debug!(target: "persist", dropped = before - after, "blob 缓存启动对账丢弃漂移项");
        }
        debug!(
            target: "persist",
            dir = %dir.display(),
            entries = index.entries.len(),
            total_bytes = index.total_bytes,
            "打开 blob 缓存"
        );
        Ok(Self {
            dir: dir.to_path_buf(),
            capacity,
            index: Mutex::new(index),
        })
    }

    /// 命中返回**文件确实存在**的绝对路径，否则 `None`(含漂移)。命中刷新 LRU。
    ///
    /// # Params:
    ///   - `key`: 缓存键(调用方给，如 SongId::qualified 或 cover URL)
    ///
    /// # Return:
    ///   命中且文件存在返回路径，否则 None。
    pub fn get(&self, key: &str) -> Option<PathBuf> {
        let mut idx = self.index.lock();
        let file = idx.entries.get(key)?.file.clone();
        let path = self.dir.join(&file);
        if !path.is_file() {
            // 漂移：索引有、文件没 → 丢该项，当 miss
            if let Some(e) = idx.entries.remove(key) {
                idx.total_bytes = idx.total_bytes.saturating_sub(e.bytes);
            }
            return None;
        }
        idx.clock += 1;
        let clock = idx.clock;
        if let Some(e) = idx.entries.get_mut(key) {
            e.last_access = clock;
        }
        Some(path)
    }

    /// 写入(覆盖同 key)。落盘到 `<subdir>/<hash>.<ext>`,成功后更新索引并按容量驱逐 LRU。
    ///
    /// `subdir` 用于分区(如来源名),让同一缓存根下不同来源/类型各占子目录;`ext` 给文件
    /// 真实扩展名,使缓存目录可被图片/音频查看器直接打开。同一 `key` 应稳定用同一 `subdir`
    /// (否则旧文件成孤儿)。
    ///
    /// # Params:
    ///   - `subdir`: 分区子目录(相对缓存根;空串则落根目录)
    ///   - `key`: 缓存键
    ///   - `data`: 内容字节
    ///   - `ext`: 文件扩展名(不含点,如 `jpg`)
    ///
    /// # Return:
    ///   写盘成功返回 `Ok(())`。
    pub fn put(&self, subdir: &str, key: &str, data: &[u8], ext: &str) -> color_eyre::Result<()> {
        let rel = Self::relative_file(subdir, key, ext);
        let path = self.dir.join(&rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .wrap_err_with(|| format!("创建 blob 子目录失败 dir={}", parent.display()))?;
        }
        std::fs::write(&path, data)
            .wrap_err_with(|| format!("写入 blob 文件失败 path={}", path.display()))?;
        let file = rel.to_string_lossy().into_owned();
        let bytes = u64::try_from(data.len())?;
        let mut idx = self.index.lock();
        idx.clock += 1;
        let clock = idx.clock;
        if let Some(old) = idx.entries.insert(
            key.to_owned(),
            Entry {
                file,
                bytes,
                last_access: clock,
            },
        ) {
            idx.total_bytes = idx.total_bytes.saturating_sub(old.bytes);
        }
        idx.total_bytes += bytes;
        self.evict_locked(&mut idx);
        Ok(())
    }

    /// 把一个**已落盘**的文件 move 入库,落到 `<subdir>/<file_name>`(调用方给可读全名,含扩展名)。
    ///
    /// 与 [`Self::put`] 的区别:`put` 收字节、hash 文件名;`put_file` 收**源路径**、用调用方
    /// 给的可读文件名,且优先 `rename`(同分区零拷贝)。供"把大文件(如音频)收编进缓存"用,
    /// 避免读进内存再写一遍。
    ///
    /// 同 `key` 已存在 → 原地覆盖其旧文件;落点 `<subdir>/<file_name>` 已被**别的 key** 占用
    /// → 追加 ` (N)` 去重(仍可读、非 hash)。
    ///
    /// # Params:
    ///   - `subdir`: 分区子目录(相对缓存根;空串则落根目录)
    ///   - `key`: 缓存键(查 index 用,需全局唯一)
    ///   - `src`: 源文件路径(成功后被移走)
    ///   - `file_name`: 落盘文件名(含扩展名,调用方负责 sanitize)
    ///
    /// # Return:
    ///   入库成功返回 `Ok(())`。
    pub fn put_file(
        &self,
        subdir: &str,
        key: &str,
        src: &Path,
        file_name: &str,
    ) -> color_eyre::Result<()> {
        // 同 key 复用原路径(原地覆盖);新 key 找一个不撞的可读名。锁只短暂持有读 own。
        let own = self.index.lock().entries.get(key).map(|e| e.file.clone());
        let rel = match own {
            Some(f) => PathBuf::from(f),
            None => self.dedup_rel(subdir, file_name),
        };
        let dst = self.dir.join(&rel);
        Self::move_into(src, &dst)?; // 不持锁,跨分区 copy 不阻塞 get
        let bytes = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
        let file = rel.to_string_lossy().into_owned();
        let mut idx = self.index.lock();
        idx.clock += 1;
        let clock = idx.clock;
        if let Some(old) = idx.entries.insert(
            key.to_owned(),
            Entry {
                file,
                bytes,
                last_access: clock,
            },
        ) {
            idx.total_bytes = idx.total_bytes.saturating_sub(old.bytes);
        }
        idx.total_bytes += bytes;
        self.evict_locked(&mut idx);
        Ok(())
    }

    /// 把 `src` move 到 `dst`:先建父目录,优先 `rename`(同分区零拷贝),跨分区退化为 copy + 删源。
    ///
    /// # Params:
    ///   - `src`: 源路径
    ///   - `dst`: 目标路径
    ///
    /// # Return:
    ///   成功返回 `Ok(())`。
    fn move_into(src: &Path, dst: &Path) -> color_eyre::Result<()> {
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)
                .wrap_err_with(|| format!("创建 blob 子目录失败 dir={}", parent.display()))?;
        }
        if std::fs::rename(src, dst).is_ok() {
            return Ok(());
        }
        // 跨分区 rename 失败 → copy + 删源(copy 会暴露真实错误)。
        std::fs::copy(src, dst).wrap_err_with(|| {
            format!("copy 入库失败 src={} dst={}", src.display(), dst.display())
        })?;
        drop(std::fs::remove_file(src));
        Ok(())
    }

    /// 为新 key 选一个不与磁盘现有文件相撞的相对路径:`<subdir>/<file_name>`,撞则追加 ` (N)`。
    ///
    /// # Params:
    ///   - `subdir`: 分区子目录
    ///   - `file_name`: 期望文件名(含扩展名)
    ///
    /// # Return:
    ///   相对缓存根、当前未被占用的路径。
    fn dedup_rel(&self, subdir: &str, file_name: &str) -> PathBuf {
        let base = PathBuf::from(subdir).join(file_name);
        if !self.dir.join(&base).exists() {
            return base;
        }
        let p = Path::new(file_name);
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(file_name);
        let ext = p.extension().and_then(|s| s.to_str());
        for n in 2u32..=9999 {
            let name = match ext {
                Some(e) => format!("{stem} ({n}).{e}"),
                None => format!("{stem} ({n})"),
            };
            let rel = PathBuf::from(subdir).join(&name);
            if !self.dir.join(&rel).exists() {
                return rel;
            }
        }
        base // 极端兜底(几乎不可能):覆盖 base,绝不 panic
    }

    /// 清空整个缓存：删所有 blob 文件 + index.bin，重置索引。供 CLI「清理缓存」用。
    ///
    /// # Return:
    ///   成功返回 `Ok(())`；单个文件删除失败不致命(尽力而为)。
    pub fn clear(&self) -> color_eyre::Result<()> {
        info!(target: "persist", dir = %self.dir.display(), "清空 blob 缓存");
        let mut idx = self.index.lock();
        for (_, e) in idx.entries.drain() {
            drop(std::fs::remove_file(self.dir.join(&e.file)));
        }
        idx.total_bytes = 0;
        idx.clock = 0;
        drop(std::fs::remove_file(self.dir.join("index.bin")));
        Ok(())
    }

    /// 把索引落到 index.bin(定期 / drop 调用)。
    ///
    /// # Return:
    ///   写盘成功返回 `Ok(())`。
    pub fn flush(&self) -> color_eyre::Result<()> {
        let idx = self.index.lock();
        let bytes = bincode::serialize(&*idx)
            .map_err(|e| color_eyre::eyre::eyre!("blob index serialize: {e}"))?;
        let path = self.dir.join("index.bin");
        std::fs::write(&path, bytes)
            .wrap_err_with(|| format!("写入 blob 索引失败 path={}", path.display()))?;
        Ok(())
    }

    /// 容量驱逐：超上限时按 last_access 升序删最旧，直到不超。
    ///
    /// # Params:
    ///   - `idx`: 已持锁的索引
    fn evict_locked(&self, idx: &mut Index) {
        while idx.total_bytes > self.capacity {
            let victim = idx
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_access)
                .map(|(k, e)| (k.clone(), e.clone()));
            let Some((key, e)) = victim else { break };
            trace!(target: "persist", key = %key, bytes = e.bytes, "驱逐缓存项");
            drop(std::fs::remove_file(self.dir.join(&e.file)));
            idx.entries.remove(&key);
            idx.total_bytes = idx.total_bytes.saturating_sub(e.bytes);
        }
    }

    /// key → 相对缓存根的文件路径 `<subdir>/<hash>.<ext>`(内容寻址)。
    ///
    /// # Params:
    ///   - `subdir`: 分区子目录(空串则无前缀)
    ///   - `key`: 缓存键
    ///   - `ext`: 扩展名(不含点)
    ///
    /// # Return:
    ///   相对路径,形如 `netease/1a2b3c4d.jpg`。
    fn relative_file(subdir: &str, key: &str, ext: &str) -> PathBuf {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        key.hash(&mut h);
        let hash = h.finish();
        PathBuf::from(subdir).join(format!("{hash:016x}.{ext}"))
    }

    /// 读 index.bin。
    ///
    /// # Params:
    ///   - `dir`: 缓存目录
    ///
    /// # Return:
    ///   反序列化出的索引;文件缺失 / 损坏返回 `Err`。
    fn load_index(dir: &Path) -> color_eyre::Result<Index> {
        let path = dir.join("index.bin");
        let bytes = std::fs::read(&path)
            .wrap_err_with(|| format!("读取 blob 索引失败 path={}", path.display()))?;
        bincode::deserialize(&bytes)
            .map_err(|e| color_eyre::eyre::eyre!("blob index deserialize: {e}"))
    }

    /// 启动对账：索引项的文件不存在 → 丢该项；孤儿文件暂留。重算 total_bytes。
    ///
    /// # Params:
    ///   - `dir`: 缓存目录
    ///   - `idx`: 待对账的索引
    fn reconcile(dir: &Path, idx: &mut Index) {
        let mut total = 0u64;
        idx.entries.retain(|_, e| {
            let ok = dir.join(&e.file).is_file();
            if ok {
                total += e.bytes;
            }
            ok
        });
        idx.total_bytes = total;
    }
}

impl Drop for BlobCache {
    fn drop(&mut self) {
        drop(self.flush());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overwrite_same_key_does_not_inflate_total() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 10)?; // 容量 10 字节
        c.put("t", "k", b"12345", "bin")?; // total = 5
        c.put("t", "k", b"67890", "bin")?; // 覆盖同 key，total 应仍是 5（不是 10）
        c.put("t", "other", b"abcde", "bin")?; // +5 → total 应为 10（不超）
        // 若覆盖时 total 膨胀到 10，这一步会让 total=15 超容量并驱逐最旧的 "k"
        assert!(
            c.get("k").is_some(),
            "k 不应被误驱逐——覆盖同 key 不该让 total_bytes 膨胀"
        );
        assert!(c.get("other").is_some());
        Ok(())
    }

    #[test]
    fn put_then_get_hits() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 1_000)?;
        c.put(/*subdir*/ "t", "k1", b"hello", /*ext*/ "bin")?;
        let Some(path) = c.get("k1") else {
            return Err(color_eyre::eyre::eyre!("miss"));
        };
        assert_eq!(std::fs::read(path)?, b"hello");
        Ok(())
    }

    /// put 落盘到 `<subdir>/<hash>.<ext>`:文件在子目录下、带真实扩展名、内容可读。
    #[test]
    fn put_places_file_under_subdir_with_ext() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 1_000)?;
        c.put(
            /*subdir*/ "netease",
            "http://x/cover",
            b"\xff\xd8\xff",
            /*ext*/ "jpg",
        )?;
        let Some(path) = c.get("http://x/cover") else {
            return Err(color_eyre::eyre::eyre!("miss"));
        };
        assert_eq!(
            path.parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str()),
            Some("netease"),
            "应落在 source 子目录下"
        );
        assert_eq!(
            path.extension().and_then(|s| s.to_str()),
            Some("jpg"),
            "扩展名应为真实图片格式"
        );
        assert_eq!(std::fs::read(&path)?, b"\xff\xd8\xff");
        Ok(())
    }

    /// 在 `dir` 下造一个内容为 `data` 的源文件,返回其路径(模拟 capture 落盘文件)。
    fn make_src(dir: &Path, name: &str, data: &[u8]) -> color_eyre::Result<PathBuf> {
        let p = dir.join(name);
        std::fs::write(&p, data)?;
        Ok(p)
    }

    /// put_file:把已落盘文件 move 入库,落到 `<subdir>/<file_name>`(可读名、无 hash),
    /// 源被移走,`get` 返回该路径且内容一致。
    #[test]
    fn put_file_moves_with_readable_name() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 1_000)?;
        let src = make_src(d.path(), "capture.bin", b"audiobytes")?;
        c.put_file(
            "netease/晴天专辑/lossless",
            "ne:1:lossless",
            &src,
            "晴天.flac",
        )?;
        let Some(path) = c.get("ne:1:lossless") else {
            return Err(color_eyre::eyre::eyre!("miss"));
        };
        assert_eq!(
            path.file_name().and_then(|s| s.to_str()),
            Some("晴天.flac"),
            "文件名应是可读原名,不是 hash"
        );
        assert!(
            path.ends_with("netease/晴天专辑/lossless/晴天.flac"),
            "应落在 subdir 下: {}",
            path.display()
        );
        assert_eq!(std::fs::read(&path)?, b"audiobytes");
        assert!(!src.exists(), "源文件应被移走");
        Ok(())
    }

    /// 同 `<subdir>/<file_name>` 被别的 key 占用 → 追加 ` (N)` 去重,两项独立可取。
    #[test]
    fn put_file_dedups_colliding_name() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 1_000)?;
        let s1 = make_src(d.path(), "a.bin", b"one")?;
        let s2 = make_src(d.path(), "b.bin", b"two")?;
        c.put_file("ne/album/lossless", "ne:1:lossless", &s1, "Track.flac")?;
        c.put_file("ne/album/lossless", "ne:2:lossless", &s2, "Track.flac")?;
        let Some(p1) = c.get("ne:1:lossless") else {
            return Err(color_eyre::eyre::eyre!("miss 1"));
        };
        let Some(p2) = c.get("ne:2:lossless") else {
            return Err(color_eyre::eyre::eyre!("miss 2"));
        };
        assert_ne!(p1, p2, "撞名应落到不同文件");
        assert_eq!(p1.file_name().and_then(|s| s.to_str()), Some("Track.flac"));
        assert_eq!(
            p2.file_name().and_then(|s| s.to_str()),
            Some("Track (2).flac"),
            "第二个应追加 (2)"
        );
        assert_eq!(std::fs::read(&p1)?, b"one");
        assert_eq!(std::fs::read(&p2)?, b"two");
        Ok(())
    }

    /// 同 key 重复 put_file → 原地覆盖、total_bytes 不膨胀。
    #[test]
    fn put_file_same_key_overwrites_in_place() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 10)?; // 容量 10
        let s1 = make_src(d.path(), "a.bin", b"12345")?;
        let s2 = make_src(d.path(), "b.bin", b"67890")?;
        c.put_file("t/lossless", "k", &s1, "x.flac")?;
        let p1 = c.get("k");
        c.put_file("t/lossless", "k", &s2, "x.flac")?;
        let p2 = c.get("k");
        assert_eq!(p1, p2, "同 key 应复用同一路径");
        c.put_file(
            "t/lossless",
            "other",
            &make_src(d.path(), "o.bin", b"abcde")?,
            "y.flac",
        )?;
        assert!(
            c.get("k").is_some(),
            "覆盖同 key 不该让 total 膨胀触发误驱逐"
        );
        assert!(c.get("other").is_some());
        if let Some(p) = p2 {
            assert_eq!(std::fs::read(&p)?, b"67890", "内容应被新值覆盖");
        }
        Ok(())
    }

    #[test]
    fn miss_returns_none() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 1_000)?;
        assert!(c.get("absent").is_none());
        Ok(())
    }

    #[test]
    fn over_capacity_evicts_lru() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 10)?;
        c.put("t", "a", b"12345", "bin")?; // 5
        c.put("t", "b", b"12345", "bin")?; // 5（共 10）
        let _ = c.get("a"); // 触碰 a → b 变最旧
        c.put("t", "cc", b"123", "bin")?; // +3 超容量 → 驱逐最旧(b)
        assert!(c.get("a").is_some());
        assert!(c.get("b").is_none());
        assert!(c.get("cc").is_some());
        Ok(())
    }

    #[test]
    fn drift_file_deleted_is_miss() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 1_000)?;
        c.put("t", "k", b"x", "bin")?;
        let Some(path) = c.get("k") else {
            return Err(color_eyre::eyre::eyre!("miss"));
        };
        std::fs::remove_file(&path)?;
        assert!(c.get("k").is_none());
        Ok(())
    }

    #[test]
    fn reopen_reconciles_index() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        {
            let c = BlobCache::open(d.path(), 1_000)?;
            c.put("t", "k", b"persisted", "bin")?;
            c.flush()?;
        }
        let c2 = BlobCache::open(d.path(), 1_000)?;
        assert!(c2.get("k").is_some());
        Ok(())
    }

    #[test]
    fn clear_removes_everything() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let c = BlobCache::open(d.path(), 1_000)?;
        c.put("t", "a", b"123", "bin")?;
        c.put("t", "b", b"456", "bin")?;
        c.clear()?;
        assert!(c.get("a").is_none());
        assert!(c.get("b").is_none());
        Ok(())
    }
}

//! DB-backed 文件缓存索引:内存镜像(sync 读)+ SQLite 写穿透(async 写)。
//!
//! 替代旧的 `BlobCache`(bincode sidecar)与 `DownloadIndex`。落盘是带可读名的原文件
//! (`<root>/<subdir>/<file_name>`,可被播放器 / 看图器直接打开);身份 → 文件的映射存进
//! SQLite 一张 `(key, relpath, bytes, last_access)` 表,**每次写当场 `UPSERT`**(无 Drop-only
//! flush、无丢数据窗口)。
//!
//! 读路径全 **sync**(内存镜像):供 `resolve_local` 在 sync 播放解析里命中本地副本。命中只是
//! 优化、永不是正确性依赖——任何漂移(文件缺失)一律当 miss,内存删项自愈;DB 里的死记录由下次
//! `open` 的启动对账清掉。`last_access`(LRU 提示)与漂移删除**只改内存、不落库**:重启后 LRU
//! 退化成近似,可接受。真正 durable 的写(入库 / 驱逐)走 async 写穿透。

use std::path::{Path, PathBuf};

use color_eyre::eyre::WrapErr;
use mineral_log::{debug, trace};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use sqlx::SqlitePool;

/// 单条索引项(内存镜像)。
struct Entry {
    /// 相对 `root` 的文件路径(`<subdir>/<file_name>`)。
    relpath: String,

    /// 文件字节数(实测 stat / 写入长度;LRU 容量核算用)。
    bytes: u64,

    /// 最近访问的逻辑时钟值(LRU 排序用;仅内存,不落库)。
    last_access: u64,
}

/// 一条被 LRU 驱逐出缓存的记录(供上层埋点 / 诊断;缓存本身不依赖它)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evicted {
    /// 被驱逐的缓存键。
    pub key: String,

    /// 释放的字节数。
    pub bytes: u64,
}

/// 内存镜像。
struct Index {
    /// key → 项。
    map: FxHashMap<String, Entry>,

    /// 当前总字节。
    total_bytes: u64,

    /// 单调逻辑时钟(每次访问 / 写入 +1)。
    clock: u64,
}

/// 真实后端(启用态)。
struct Backend {
    /// sqlite 连接池(daemon 复用 `mineral.db`;client 自开 `tui.db`)。
    pool: SqlitePool,

    /// 索引表名(我方选定的静态名,直接拼进 SQL;非外部输入,无注入)。
    table: &'static str,

    /// 文件根目录;`relpath` 相对它,`get` 返回 `root.join(relpath)`。
    root: PathBuf,

    /// 容量上限(字节);`None` = 不驱逐(永久导出索引)。
    capacity: Option<u64>,

    /// 内存镜像。
    index: Mutex<Index>,
}

/// DB-backed 文件缓存索引。线程安全(内部 `Mutex`);`get` / `forget` sync,写穿透 async。
pub struct CacheIndex {
    /// 后端;`None` = 降级(`get` 恒 miss、写 no-op、不依赖 DB / runtime)。
    backend: Option<Backend>,
}

/// 缓存内容的只读统计快照(供 CLI 展示 / 清理回执)。
///
/// `entries` 的 `relpath` 语义(来源 / 音质 / 格式)由写入方的落盘布局决定,
/// [`CacheIndex`] 不解析;消费方据 `root` + `relpath` 自行 stat(如取 mtime)。
///
/// 只读返回 DTO:字段全 `pub` 供消费方读 / 在测试中构造,不是配置 struct。
pub struct CacheStats {
    /// 文件根目录;`None` = 降级态(无后端)。
    pub root: Option<PathBuf>,

    /// 每条目的相对路径与字节数。
    pub entries: Vec<CacheEntryStat>,

    /// 当前总字节(= `entries` 字节之和)。
    pub total_bytes: u64,

    /// 容量上限(字节);`None` = 不驱逐。
    pub capacity: Option<u64>,
}

/// 单条缓存项的统计。只读返回 DTO,字段全 `pub`。
pub struct CacheEntryStat {
    /// 相对 `root` 的文件路径(`<subdir>/<file_name>`)。
    pub relpath: String,

    /// 文件字节数。
    pub bytes: u64,
}

impl CacheIndex {
    /// 在给定连接池上打开(或新建)一张索引表并载入内存镜像 + 启动对账。
    ///
    /// # Params:
    ///   - `pool`: sqlite 连接池(调用方持有 / 复用)
    ///   - `table`: 索引表名(我方静态选定)
    ///   - `root`: 文件根目录
    ///   - `capacity`: 容量上限字节;`None` 不驱逐
    ///
    /// # Return:
    ///   就绪索引;建表 / 载入失败返回 `Err`(调用方可降级到 [`Self::disabled`])。
    pub async fn open(
        pool: SqlitePool,
        table: &'static str,
        root: PathBuf,
        capacity: Option<u64>,
    ) -> color_eyre::Result<Self> {
        sqlx::query(&format!(
            "CREATE TABLE IF NOT EXISTS {table} \
             (key TEXT PRIMARY KEY, relpath TEXT NOT NULL, \
              bytes INTEGER NOT NULL, last_access INTEGER NOT NULL)"
        ))
        .execute(&pool)
        .await
        .wrap_err_with(|| format!("建缓存索引表失败 table={table}"))?;

        let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(&format!(
            "SELECT key, relpath, bytes, last_access FROM {table}"
        ))
        .fetch_all(&pool)
        .await
        .wrap_err_with(|| format!("载入缓存索引失败 table={table}"))?;

        let backend = Backend {
            pool,
            table,
            root,
            capacity,
            index: Mutex::new(Index {
                map: FxHashMap::default(),
                total_bytes: 0,
                clock: 0,
            }),
        };
        backend.reconcile(rows).await?;
        debug!(
            target: "persist",
            table,
            entries = backend.index.lock().map.len(),
            "缓存索引就绪"
        );
        Ok(Self {
            backend: Some(backend),
        })
    }

    /// 降级索引:`get` 恒空、`forget` / 写入 no-op,不依赖 DB / tokio runtime。
    ///
    /// # Return:
    ///   一个永远成功但无副作用的索引。
    pub fn disabled() -> Self {
        Self { backend: None }
    }

    /// 命中返回**文件确实存在**的绝对路径,否则 `None`(含漂移)。命中刷新 LRU(仅内存)。
    ///
    /// # Params:
    ///   - `key`: 缓存键(调用方给,需全局唯一)
    ///
    /// # Return:
    ///   命中且文件存在返回 `root.join(relpath)`,否则 `None`。
    pub fn get(&self, key: &str) -> Option<PathBuf> {
        let backend = self.backend.as_ref()?;
        let mut idx = backend.index.lock();
        let relpath = idx.map.get(key)?.relpath.clone();
        let path = backend.root.join(&relpath);
        if !path.is_file() {
            // 漂移:索引有、文件没 → 内存删项当 miss(DB 死记录留待下次 open 对账)。
            if let Some(e) = idx.map.remove(key) {
                idx.total_bytes = idx.total_bytes.saturating_sub(e.bytes);
            }
            return None;
        }
        idx.clock = idx.clock.saturating_add(1);
        let clock = idx.clock;
        if let Some(e) = idx.map.get_mut(key) {
            e.last_access = clock;
        }
        Some(path)
    }

    /// 删一条记录(仅内存,best-effort 自愈;DB 留待下次 `open` 对账)。
    ///
    /// # Params:
    ///   - `key`: 缓存键
    pub fn forget(&self, key: &str) {
        let Some(backend) = self.backend.as_ref() else {
            return;
        };
        let mut idx = backend.index.lock();
        if let Some(e) = idx.map.remove(key) {
            idx.total_bytes = idx.total_bytes.saturating_sub(e.bytes);
        }
    }

    /// 登记一条**调用方已置好文件**的记录(覆盖同 key)并写穿透落库;超容量则驱逐。
    /// 供永久导出(文件已落在 `root` 下)用。
    ///
    /// # Params:
    ///   - `key`: 缓存键
    ///   - `relpath`: 相对 `root` 的文件路径(含扩展名)
    ///   - `bytes`: 文件字节数(实测;别拿 0 冒充「未核算」,容量核算与状态报表都信它)
    ///
    /// # Return:
    ///   写盘成功 / 降级返回 `Ok(())`。
    pub async fn record(&self, key: &str, relpath: &str, bytes: u64) -> color_eyre::Result<()> {
        let Some(backend) = self.backend.as_ref() else {
            return Ok(());
        };
        let last_access = backend.insert_mirror(key, relpath.to_owned(), bytes);
        backend.upsert_row(key, relpath, bytes, last_access).await?;
        backend.evict_to_capacity().await.map(drop)
    }

    /// 把一个**已落盘**的源文件 move 入库(落到 `<root>/<subdir>/<file_name>`,撞名追加 ` (N)`),
    /// 再写穿透落库;超容量则驱逐。供缓存入库(边播边 capture 后收编)用。
    ///
    /// 同 key 已存在 → 复用原 `relpath` 原地覆盖;落点被**别的 key** 占用 → ` (N)` 去重。
    /// 文件 move(可能跨分区大拷贝)在 `spawn_blocking` 上做,不阻塞执行器。
    ///
    /// # Params:
    ///   - `key`: 缓存键(全局唯一)
    ///   - `src`: 源文件路径(成功后被移走)
    ///   - `subdir`: 分区子目录(相对 `root`)
    ///   - `file_name`: 落盘文件名(含扩展名,调用方负责 sanitize)
    ///
    /// # Return:
    ///   入库成功返回本次 LRU 驱逐掉的记录(可空;供上层埋点);降级返回空 vec。
    pub async fn record_file(
        &self,
        key: &str,
        src: &Path,
        subdir: &str,
        file_name: &str,
    ) -> color_eyre::Result<Vec<Evicted>> {
        let Some(backend) = self.backend.as_ref() else {
            return Ok(Vec::new());
        };
        let existing = backend.index.lock().map.get(key).map(|e| e.relpath.clone());
        let root = backend.root.clone();
        let subdir = subdir.to_owned();
        let file_name = file_name.to_owned();
        let src = src.to_path_buf();
        let (relpath, bytes) = tokio::task::spawn_blocking(move || {
            place_file(&root, &subdir, &file_name, &src, existing)
        })
        .await
        .wrap_err("入库文件 move 任务 join 失败")??;

        let last_access = backend.insert_mirror(key, relpath.clone(), bytes);
        backend
            .upsert_row(key, &relpath, bytes, last_access)
            .await?;
        backend.evict_to_capacity().await
    }

    /// 把一段**内存字节**写入库(落到 `<root>/<subdir>/<file_name>`,撞名追加 ` (N)`)再写穿透落库;
    /// 超容量则驱逐。供"手上是字节、不是文件"的场景(如封面)用,免去先写临时文件再 move。
    ///
    /// 同 key 复用原 `relpath` 原地覆盖;落点被别的 key 占用 → ` (N)` 去重。写盘在 `spawn_blocking`。
    ///
    /// # Params:
    ///   - `key`: 缓存键(全局唯一)
    ///   - `data`: 落盘字节
    ///   - `subdir`: 分区子目录(相对 `root`)
    ///   - `file_name`: 落盘文件名(含扩展名,调用方负责 sanitize / 唯一化)
    ///
    /// # Return:
    ///   入库成功 / 降级返回 `Ok(())`。
    pub async fn put_bytes(
        &self,
        key: &str,
        data: &[u8],
        subdir: &str,
        file_name: &str,
    ) -> color_eyre::Result<()> {
        let Some(backend) = self.backend.as_ref() else {
            return Ok(());
        };
        let existing = backend.index.lock().map.get(key).map(|e| e.relpath.clone());
        let root = backend.root.clone();
        let subdir = subdir.to_owned();
        let file_name = file_name.to_owned();
        let data = data.to_vec();
        let (relpath, bytes) = tokio::task::spawn_blocking(move || {
            write_bytes_file(&root, &subdir, &file_name, &data, existing)
        })
        .await
        .wrap_err("入库字节写盘任务 join 失败")??;

        let last_access = backend.insert_mirror(key, relpath.clone(), bytes);
        backend
            .upsert_row(key, &relpath, bytes, last_access)
            .await?;
        backend.evict_to_capacity().await.map(drop)
    }

    /// 当前索引内容的只读快照(读内存镜像,不触盘)。供 CLI `cache status` 展示用。
    ///
    /// # Return:
    ///   启用态返回各条目 + 总字节 + 容量;降级态返回空快照(`root` / `capacity` 均 `None`)。
    pub fn snapshot(&self) -> CacheStats {
        let Some(backend) = self.backend.as_ref() else {
            return CacheStats {
                root: None,
                entries: Vec::new(),
                total_bytes: 0,
                capacity: None,
            };
        };
        let idx = backend.index.lock();
        let entries = idx
            .map
            .values()
            .map(|e| CacheEntryStat {
                relpath: e.relpath.clone(),
                bytes: e.bytes,
            })
            .collect::<Vec<_>>();
        CacheStats {
            root: Some(backend.root.clone()),
            entries,
            total_bytes: idx.total_bytes,
            capacity: backend.capacity,
        }
    }

    /// 清空整张索引:删所有文件 + `DELETE FROM <table>` + 清镜像。供 CLI「清理缓存」用。
    ///
    /// # Return:
    ///   成功返回清理前的内容快照(条目 / 总字节 = 释放量);单文件删除失败不致命(尽力而为)。
    ///   降级态返回空快照。
    pub async fn clear(&self) -> color_eyre::Result<CacheStats> {
        let Some(backend) = self.backend.as_ref() else {
            return Ok(CacheStats {
                root: None,
                entries: Vec::new(),
                total_bytes: 0,
                capacity: None,
            });
        };
        let (entries, total_bytes) = {
            let mut idx = backend.index.lock();
            let total_bytes = idx.total_bytes;
            idx.total_bytes = 0;
            idx.clock = 0;
            let entries = idx
                .map
                .drain()
                .map(|(_, e)| CacheEntryStat {
                    relpath: e.relpath,
                    bytes: e.bytes,
                })
                .collect::<Vec<_>>();
            (entries, total_bytes)
        };
        for entry in &entries {
            drop(std::fs::remove_file(backend.root.join(&entry.relpath)));
        }
        sqlx::query(&format!("DELETE FROM {}", backend.table))
            .execute(&backend.pool)
            .await
            .wrap_err_with(|| format!("清空缓存索引表失败 table={}", backend.table))?;
        Ok(CacheStats {
            root: Some(backend.root.clone()),
            entries,
            total_bytes,
            capacity: backend.capacity,
        })
    }
}

impl Backend {
    /// 启动对账:逐条 stat `root/relpath`,文件在 → 收进镜像、累加 `total_bytes`、抬高 `clock`;
    /// 文件不在 → 丢该项并从 DB `DELETE`(在 async open 里 await)。
    ///
    /// # Params:
    ///   - `rows`: 从 DB 载入的 `(key, relpath, bytes, last_access)`
    ///
    /// # Return:
    ///   对账成功返回 `Ok(())`。
    async fn reconcile(&self, rows: Vec<(String, String, i64, i64)>) -> color_eyre::Result<()> {
        let mut dead = Vec::<String>::new();
        {
            let mut idx = self.index.lock();
            for (key, relpath, bytes, last_access) in rows {
                if !self.root.join(&relpath).is_file() {
                    dead.push(key);
                    continue;
                }
                let bytes = u64::try_from(bytes).unwrap_or(0);
                let last_access = u64::try_from(last_access).unwrap_or(0);
                idx.total_bytes = idx.total_bytes.saturating_add(bytes);
                idx.clock = idx.clock.max(last_access);
                idx.map.insert(
                    key,
                    Entry {
                        relpath,
                        bytes,
                        last_access,
                    },
                );
            }
        }
        for key in dead {
            self.delete_row(&key).await?;
        }
        Ok(())
    }

    /// 把一条记录写进内存镜像(覆盖同 key 不膨胀 `total_bytes`),返回分配的 `last_access`。
    ///
    /// # Params:
    ///   - `key`: 缓存键
    ///   - `relpath`: 相对路径
    ///   - `bytes`: 字节数
    ///
    /// # Return:
    ///   本次写入分配的 `last_access`(供随后落库)。
    fn insert_mirror(&self, key: &str, relpath: String, bytes: u64) -> u64 {
        let mut idx = self.index.lock();
        idx.clock = idx.clock.saturating_add(1);
        let last_access = idx.clock;
        if let Some(old) = idx.map.insert(
            key.to_owned(),
            Entry {
                relpath,
                bytes,
                last_access,
            },
        ) {
            idx.total_bytes = idx.total_bytes.saturating_sub(old.bytes);
        }
        idx.total_bytes = idx.total_bytes.saturating_add(bytes);
        last_access
    }

    /// `UPSERT` 一行(写穿透)。
    async fn upsert_row(
        &self,
        key: &str,
        relpath: &str,
        bytes: u64,
        last_access: u64,
    ) -> color_eyre::Result<()> {
        trace!(target: "persist", table = self.table, key, "缓存索引 upsert");
        sqlx::query(&format!(
            "INSERT INTO {}(key, relpath, bytes, last_access) VALUES(?,?,?,?) \
             ON CONFLICT(key) DO UPDATE SET \
               relpath=excluded.relpath, bytes=excluded.bytes, last_access=excluded.last_access",
            self.table
        ))
        .bind(key)
        .bind(relpath)
        .bind(i64::try_from(bytes)?)
        .bind(i64::try_from(last_access)?)
        .execute(&self.pool)
        .await
        .wrap_err_with(|| format!("缓存索引 upsert 失败 table={} key={key}", self.table))?;
        Ok(())
    }

    /// `DELETE` 一行。
    async fn delete_row(&self, key: &str) -> color_eyre::Result<()> {
        sqlx::query(&format!("DELETE FROM {} WHERE key=?", self.table))
            .bind(key)
            .execute(&self.pool)
            .await
            .wrap_err_with(|| format!("缓存索引 delete 失败 table={} key={key}", self.table))?;
        Ok(())
    }

    /// 超容量驱逐:按 `last_access` 升序删最旧(删文件 + `DELETE` 行 + 改 `total_bytes`),
    /// 直到不超。`capacity` 为 `None` 时直接返回。锁不跨 await。
    ///
    /// # Return:
    ///   驱逐完成返回 `Ok(())`。
    async fn evict_to_capacity(&self) -> color_eyre::Result<Vec<Evicted>> {
        let mut evicted = Vec::<Evicted>::new();
        let Some(cap) = self.capacity else {
            return Ok(evicted);
        };
        loop {
            let victim = {
                let mut idx = self.index.lock();
                if idx.total_bytes <= cap {
                    return Ok(evicted);
                }
                let pick = idx
                    .map
                    .iter()
                    .min_by_key(|(_, e)| e.last_access)
                    .map(|(k, e)| (k.clone(), e.relpath.clone(), e.bytes));
                match pick {
                    Some((key, relpath, bytes)) => {
                        idx.map.remove(&key);
                        idx.total_bytes = idx.total_bytes.saturating_sub(bytes);
                        (key, relpath, bytes)
                    }
                    None => return Ok(evicted),
                }
            };
            let (key, relpath, bytes) = victim;
            drop(std::fs::remove_file(self.root.join(&relpath)));
            self.delete_row(&key).await?;
            evicted.push(Evicted { key, bytes });
        }
    }
}

/// 为入库文件选不撞名的相对路径并把 `src` move 过去,返回 `(relpath, bytes)`。
/// 同 key 复用 `existing`(原地覆盖);否则 `<subdir>/<file_name>`,撞盘则追加 ` (N)`。
///
/// # Params:
///   - `root`: 文件根目录
///   - `subdir`: 分区子目录
///   - `file_name`: 期望文件名(含扩展名)
///   - `src`: 源文件路径
///   - `existing`: 同 key 旧 relpath(有则复用)
///
/// # Return:
///   `(相对路径, move 后文件字节数)`。
fn place_file(
    root: &Path,
    subdir: &str,
    file_name: &str,
    src: &Path,
    existing: Option<String>,
) -> color_eyre::Result<(String, u64)> {
    let rel = match existing {
        Some(r) => r,
        None => dedup_rel(root, subdir, file_name),
    };
    let dst = root.join(&rel);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("创建缓存子目录失败 dir={}", parent.display()))?;
    }
    if std::fs::rename(src, &dst).is_err() {
        // 跨分区 rename 失败 → copy + 删源(copy 暴露真实错误)。
        std::fs::copy(src, &dst).wrap_err_with(|| {
            format!("copy 入库失败 src={} dst={}", src.display(), dst.display())
        })?;
        drop(std::fs::remove_file(src));
    }
    // 刚落盘就 stat 不到 = 真实 IO 异常,冒泡而非把 0 当字节数记进索引(容量核算会失真)。
    let bytes = std::fs::metadata(&dst)
        .map(|m| m.len())
        .wrap_err_with(|| format!("stat 入库文件失败 path={}", dst.display()))?;
    Ok((rel, bytes))
}

/// 把 `data` 写到不撞名的相对路径并返回 `(relpath, bytes)`。同 key 复用 `existing`(原地覆盖);
/// 否则 `<subdir>/<file_name>`,撞盘则追加 ` (N)`。
///
/// # Params:
///   - `root`: 文件根目录
///   - `subdir`: 分区子目录
///   - `file_name`: 期望文件名(含扩展名)
///   - `data`: 落盘字节
///   - `existing`: 同 key 旧 relpath(有则复用)
///
/// # Return:
///   `(相对路径, 字节数)`。
fn write_bytes_file(
    root: &Path,
    subdir: &str,
    file_name: &str,
    data: &[u8],
    existing: Option<String>,
) -> color_eyre::Result<(String, u64)> {
    let rel = match existing {
        Some(r) => r,
        None => dedup_rel(root, subdir, file_name),
    };
    let dst = root.join(&rel);
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .wrap_err_with(|| format!("创建缓存子目录失败 dir={}", parent.display()))?;
    }
    std::fs::write(&dst, data)
        .wrap_err_with(|| format!("写缓存文件失败 path={}", dst.display()))?;
    Ok((rel, u64::try_from(data.len())?))
}

/// 为新 key 选一个不与磁盘现有文件相撞的相对路径:`<subdir>/<file_name>`,撞则追加 ` (N)`。
///
/// # Params:
///   - `root`: 根目录
///   - `subdir`: 分区子目录
///   - `file_name`: 期望文件名(含扩展名)
///
/// # Return:
///   相对 `root`、当前未被占用的路径(`/` 分隔)。
fn dedup_rel(root: &Path, subdir: &str, file_name: &str) -> String {
    let first = format!("{subdir}/{file_name}");
    if !root.join(&first).exists() {
        return first;
    }
    let p = Path::new(file_name);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or(file_name);
    let ext = p.extension().and_then(|s| s.to_str());
    for n in 2u32..=9999 {
        let name = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let rel = format!("{subdir}/{name}");
        if !root.join(&rel).exists() {
            return rel;
        }
    }
    first // 极端兜底(几乎不可能):覆盖 first,绝不 panic
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use sqlx::SqlitePool;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::{CacheIndex, Evicted};

    /// 开一个内存 sqlite 池(每个测试独立)。
    async fn mem_pool() -> color_eyre::Result<SqlitePool> {
        Ok(SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await?)
    }

    /// 在 `dir` 下造一个内容为 `data` 的源文件(模拟 capture 落盘),返回其路径。
    fn make_src(dir: &Path, name: &str, data: &[u8]) -> color_eyre::Result<std::path::PathBuf> {
        let p = dir.join(name);
        std::fs::write(&p, data)?;
        Ok(p)
    }

    /// record_file 入库 → get 命中可读名;源被移走;内容一致。
    #[tokio::test]
    async fn record_file_then_get() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("root");
        let idx = CacheIndex::open(mem_pool().await?, "audio_cache", root, Some(1_000_000)).await?;
        let src = make_src(d.path(), "cap.part", b"AUDIO")?;
        idx.record_file("ne:1:exhigh", &src, "netease/exhigh/专辑", "晴天.mp3")
            .await?;
        let Some(path) = idx.get("ne:1:exhigh") else {
            return Err(color_eyre::eyre::eyre!("入库后应命中"));
        };
        assert!(path.ends_with("netease/exhigh/专辑/晴天.mp3"), "可读库路径");
        assert_eq!(std::fs::read(&path)?, b"AUDIO");
        assert!(!src.exists(), "源应被移走");
        Ok(())
    }

    /// 核心回归:写入后**不靠 Drop**,另起实例从同 DB + 同盘 open 仍命中(就是本次 bug 的修复点)。
    #[tokio::test]
    async fn survives_reopen_without_drop() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("root");
        let pool = mem_pool().await?;
        {
            let idx = CacheIndex::open(pool.clone(), "audio_cache", root.clone(), Some(1_000_000))
                .await?;
            let src = make_src(d.path(), "cap.part", b"AUDIO")?;
            idx.record_file("ne:1:exhigh", &src, "netease/exhigh/x", "a.mp3")
                .await?;
            // 不 flush、不 drop 即"重开"(同池模拟进程内换实例;真实是新进程读同文件)。
        }
        let reopened = CacheIndex::open(pool, "audio_cache", root, Some(1_000_000)).await?;
        assert!(
            reopened.get("ne:1:exhigh").is_some(),
            "写穿透后重开应仍命中,不再依赖 Drop flush"
        );
        Ok(())
    }

    /// 漂移:文件被删 → get miss,且内存项被清。
    #[tokio::test]
    async fn drift_is_miss() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("root");
        let idx = CacheIndex::open(mem_pool().await?, "audio_cache", root, Some(1_000_000)).await?;
        let src = make_src(d.path(), "cap.part", b"X")?;
        idx.record_file("k", &src, "sub", "a.mp3").await?;
        let Some(path) = idx.get("k") else {
            return Err(color_eyre::eyre::eyre!("应命中"));
        };
        std::fs::remove_file(&path)?;
        assert!(idx.get("k").is_none(), "文件没了应 miss");
        Ok(())
    }

    /// LRU 驱逐:超容量删最旧(触碰过的留下)。
    #[tokio::test]
    async fn evicts_lru_over_capacity() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("root");
        // 容量 10 字节。
        let idx = CacheIndex::open(mem_pool().await?, "audio_cache", root, Some(10)).await?;
        idx.record_file("a", &make_src(d.path(), "a", b"12345")?, "s", "a.bin")
            .await?; // 5
        idx.record_file("b", &make_src(d.path(), "b", b"12345")?, "s", "b.bin")
            .await?; // 共 10
        let _ = idx.get("a"); // 触碰 a → b 变最旧
        idx.record_file("c", &make_src(d.path(), "c", b"123")?, "s", "c.bin")
            .await?; // +3 超 → 驱逐最旧 b
        assert!(idx.get("a").is_some());
        assert!(idx.get("b").is_none(), "最旧 b 应被驱逐");
        assert!(idx.get("c").is_some());
        Ok(())
    }

    /// record_file 返回本次 LRU 驱逐掉的记录(供 cache_evictions 埋点):驱逐 b(5 字节)。
    #[tokio::test]
    async fn record_file_returns_evicted_entries() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("root");
        let idx = CacheIndex::open(mem_pool().await?, "audio_cache", root, Some(10)).await?;
        let first = idx
            .record_file("a", &make_src(d.path(), "a", b"12345")?, "s", "a.bin")
            .await?;
        assert!(first.is_empty(), "首入未超容量,无驱逐");
        idx.record_file("b", &make_src(d.path(), "b", b"12345")?, "s", "b.bin")
            .await?; // 共 10
        let _ = idx.get("a"); // 触碰 a → b 变最旧
        let evicted = idx
            .record_file("c", &make_src(d.path(), "c", b"123")?, "s", "c.bin")
            .await?; // +3 超 → 驱逐最旧 b
        assert_eq!(
            evicted,
            vec![Evicted {
                key: "b".to_owned(),
                bytes: 5
            }],
            "应返回被驱逐的 b(5 字节)"
        );
        Ok(())
    }

    /// 撞名:同落点被别的 key 占 → ` (N)` 去重,两项独立可取。
    #[tokio::test]
    async fn dedups_colliding_name() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("root");
        let idx = CacheIndex::open(mem_pool().await?, "audio_cache", root, Some(1_000_000)).await?;
        idx.record_file("k1", &make_src(d.path(), "a", b"one")?, "s", "T.mp3")
            .await?;
        idx.record_file("k2", &make_src(d.path(), "b", b"two")?, "s", "T.mp3")
            .await?;
        let (Some(p1), Some(p2)) = (idx.get("k1"), idx.get("k2")) else {
            return Err(color_eyre::eyre::eyre!("两项都应命中"));
        };
        assert_ne!(p1, p2, "撞名应落不同文件");
        assert_eq!(p2.file_name().and_then(|s| s.to_str()), Some("T (2).mp3"));
        Ok(())
    }

    /// record(文件已置)+ 不驱逐(capacity None,永久副本语义)。
    #[tokio::test]
    async fn record_path_no_evict() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("root");
        std::fs::create_dir_all(root.join("sub"))?;
        std::fs::write(root.join("sub/x.flac"), b"BIGFLAC")?;
        let idx = CacheIndex::open(mem_pool().await?, "no_evict", root, None).await?;
        idx.record("ne:1:lossless", "sub/x.flac", 7).await?;
        assert!(idx.get("ne:1:lossless").is_some());
        Ok(())
    }

    /// 降级实例:get 恒空、写 no-op 且不报错。
    #[tokio::test]
    async fn disabled_is_null_object() -> color_eyre::Result<()> {
        let idx = CacheIndex::disabled();
        assert!(idx.get("k").is_none());
        idx.forget("k");
        idx.record("k", "x", 0).await?;
        assert!(idx.get("k").is_none());
        Ok(())
    }

    /// clear:删文件 + 清表 + 清镜像;返回清理前的内容快照(条目 + 释放字节)。
    #[tokio::test]
    async fn clear_removes_everything() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("root");
        let idx = CacheIndex::open(mem_pool().await?, "audio_cache", root, Some(1_000_000)).await?;
        idx.record_file("k", &make_src(d.path(), "a", b"hello")?, "s", "a.bin")
            .await?;
        let removed = idx.clear().await?;
        assert_eq!(removed.entries.len(), 1, "应回执 1 条被清条目");
        assert_eq!(removed.total_bytes, 5, "释放字节 = 文件大小");
        assert_eq!(
            removed.entries.first().map(|e| e.relpath.as_str()),
            Some("s/a.bin")
        );
        assert!(idx.get("k").is_none());
        assert_eq!(idx.snapshot().total_bytes, 0, "清后快照应为空");
        Ok(())
    }

    /// snapshot:只读返回当前条目 / 总字节 / 容量,且与实际写入吻合。
    #[tokio::test]
    async fn snapshot_reports_entries_and_capacity() -> color_eyre::Result<()> {
        let d = tempfile::tempdir()?;
        let root = d.path().join("root");
        let idx = CacheIndex::open(mem_pool().await?, "audio_cache", root, Some(1_000_000)).await?;
        idx.record_file("k1", &make_src(d.path(), "a", b"123")?, "s", "a.bin")
            .await?;
        idx.record_file("k2", &make_src(d.path(), "b", b"45")?, "s", "b.bin")
            .await?;
        let snap = idx.snapshot();
        assert_eq!(snap.entries.len(), 2);
        assert_eq!(snap.total_bytes, 5, "3 + 2 字节");
        assert_eq!(snap.capacity, Some(1_000_000));
        assert!(snap.root.is_some());
        Ok(())
    }

    /// 降级态 snapshot / clear 均返回空快照,不触盘不报错。
    #[tokio::test]
    async fn disabled_snapshot_is_empty() -> color_eyre::Result<()> {
        let idx = CacheIndex::disabled();
        let snap = idx.snapshot();
        assert!(snap.root.is_none());
        assert!(snap.entries.is_empty());
        assert_eq!(snap.total_bytes, 0);
        assert_eq!(snap.capacity, None);
        let removed = idx.clear().await?;
        assert!(removed.entries.is_empty());
        Ok(())
    }
}

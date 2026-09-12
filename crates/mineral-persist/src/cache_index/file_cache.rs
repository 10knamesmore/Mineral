//! 文件缓存的内存索引、SQLite 写穿透、启动对账与 LRU 驱逐。

use std::path::{Path, PathBuf};

use color_eyre::eyre::WrapErr;
use mineral_log::{debug, trace};
use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use sea_orm::DatabaseConnection;

use super::file_placement::{place_file, write_bytes_file};
use super::storage::{CacheRow, CacheTable};

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
    pool: DatabaseConnection,

    /// 缓存用途，映射到对应的持久化实体。
    table: CacheTable,

    /// 文件根目录;`relpath` 相对它,`get` 返回 `root.join(relpath)`。
    root: PathBuf,

    /// 容量上限(字节);`None` = 不驱逐。
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
    ///   - `table`: 缓存用途
    ///   - `root`: 文件根目录
    ///   - `capacity`: 容量上限字节;`None` 不驱逐
    ///
    /// # Return:
    ///   就绪索引;建表 / 载入失败返回 `Err`(调用方可降级到 [`Self::disabled`])。
    pub(crate) async fn open(
        pool: DatabaseConnection,
        table: CacheTable,
        root: PathBuf,
        capacity: Option<u64>,
    ) -> color_eyre::Result<Self> {
        let rows = table
            .load(&pool)
            .await
            .wrap_err_with(|| format!("载入缓存索引失败 table={}", table.name()))?;

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
            table = table.name(),
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
        backend
            .table
            .clear(&backend.pool)
            .await
            .wrap_err_with(|| format!("清空缓存索引表失败 table={}", backend.table.name()))?;
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
    async fn reconcile(&self, rows: Vec<CacheRow>) -> color_eyre::Result<()> {
        let mut dead = Vec::<String>::new();
        {
            let mut idx = self.index.lock();
            for CacheRow {
                key,
                relpath,
                bytes,
                last_access,
            } in rows
            {
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
        trace!(target: "persist", table = self.table.name(), key, "缓存索引 upsert");
        self.table
            .upsert(
                &self.pool,
                CacheRow {
                    key: key.to_owned(),
                    relpath: relpath.to_owned(),
                    bytes: i64::try_from(bytes)?,
                    last_access: i64::try_from(last_access)?,
                },
            )
            .await
            .wrap_err_with(|| format!("缓存索引 upsert 失败 table={} key={key}", self.table.name()))
    }

    /// `DELETE` 一行。
    async fn delete_row(&self, key: &str) -> color_eyre::Result<()> {
        self.table
            .delete(&self.pool, key)
            .await
            .wrap_err_with(|| format!("缓存索引 delete 失败 table={} key={key}", self.table.name()))
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

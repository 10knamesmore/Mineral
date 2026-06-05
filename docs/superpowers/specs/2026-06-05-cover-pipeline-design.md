# 封面管线优化:缓存存加工后产物 + 64px 取色(设计已确认,待实现)

> 状态:2026-06-05 设计已经用户逐项确认,**未实现**。本文件仅存本地,不入 git。
> 背景:perf 第二轮采样(PlayerSync 落地后),封面管线占 TUI 全部 CPU ~45%。

## 1. 根因链(已用 perf + 代码双重确认)

- `prefetch.rs:21` `RADIUS = 64`:浏览选中 ±64 张封面每 tick 预取 → 启动/滚动一口气上百张(**RADIUS 不动**,产品决策,锅在单张成本);
- `cover_fetch.rs:63` `COVER_STORAGE = CoverStorageMode::Raw`:磁盘缓存存**原始下载字节**(网易常为 1024²);
- 每张缓存命中 = 读盘 1024² JPEG → zune 解码 → `decode_resize` 缩到 384(`COVER_MAX_DIM`,cover_fetch.rs:43)→ 在 384²=14.7 万像素上跑 k-means 20 轮(`cover_colors.rs:49`)≈ **45ms/张**;
- 实测:启动 6s 窗 36.1G cycles ≈ 1.7 核跑满(占全程 66%),`resize_exact` 23.5% + `extract_palette` 12% + zune_jpeg ~8%,稳态滚动窗残余 resize 8~15%。

注意:perf 里的 `DynamicImage::resize_exact` 主要来自 fetcher 的 `decode_resize`
(`img.resize` 内部走 resize_exact);encode 侧(cover_encode.rs,384→面板像素
+base64)已是离线 worker、按需触发,**不在本次范围**。

## 2. 方案(三件事)

### 2.1 `COVER_STORAGE`:`Raw` → `Resized`(一行)

`cover_fetch.rs:63` 常量切换。`Resized` 模式(`decode_resize` 后重编码 384px JPEG
q85)**已完整实现并有测试**(`bytes_for_cache_resized_reencodes_jpeg_within_max_dim`)。
切换后缓存命中 = 解一张 ≤384 JPEG(像素 1/7),`decode_resize` 的 resize 分支变 no-op。

实现细节:`#[allow(dead_code)]` 目前挂在 `Resized` variant 上(cover_fetch.rs:58,
因未被构造);切换后改挂到 `Raw` 上并更新注释(Raw 留给将来配置系统选用)。

### 2.2 旧缓存自愈升级

存量缓存全是 Raw 原图,命中路径(`fetch_and_decode` cover_fetch.rs:248 的
`cached_read` 早返回分支)直接解码返回,永远不会变小。改动:

- `decode_resize`(cover_fetch.rs:360)返回值带上 `clamped: bool`(原图超过
  `COVER_MAX_DIM` 被缩过)。建议小 struct(如 `DecodedImage { image, clamped }`),
  不要裸 tuple bool(CLAUDE.md 禁谜语参数);`DecodedCover` 顺势带 `clamped`。
- Remote 命中分支:`clamped && matches!(COVER_STORAGE, Resized)` 时,在 blocking
  池里 `bytes_for_cache(COVER_STORAGE, &bytes, &image)` 重编码,经
  `store_best_effort`(cover_fetch.rs:418)**覆盖回写**同 key。每个旧条目只触发一次
  (升级后不再 clamped),缓存逐渐自愈,用户无需清缓存。

**已确认的持久层语义**(mineral-persist cache_index.rs:277 `put_bytes`):同 key
复用原 `relpath` **原地覆盖**。⚠️ 这意味着旧条目扩展名不变(.png 文件里躺着 jpeg
字节)——无害,读回解码按字节嗅探不信后缀(`sniff_ext` 同理),但实现/测试别假设
回写后扩展名更新。

### 2.3 k-means 在 64×64 缩略图上跑

`cover_colors.rs:49` `extract_palette` 入口:图大于 64 时先 `img.thumbnail(64, 64)`
(box filter,极快)再转 Lab 聚类。14.7 万像素 → ~4 千,**~36 倍降本**(连带
perf 里的 `roundf`/`cbrtf` 热点)。固定 seed(`COVER_KMEANS_SEED`)+ thumbnail
确定性 → 取色仍确定;色板数值与旧版略有差异(聚类输入变了),频谱配色视觉无感。
现有测试用 60×60 图,小于 64 不受影响。建议常量名 `PALETTE_SAMPLE_DIM: u32 = 64`。

## 3. 明确不做(已与用户确认)

- **色板持久化**:2.3 之后 k-means <1ms,而图反正要解码显示;为最后 1ms 加
  sidecar/DB 迁移不值(YAGNI)。
- **`RADIUS = 64` 不降**:预取本身是对的。
- **`COVER_MAX_DIM` 不动(384)**:用户问过"能否直接显示原图"——结论是不行
  (内存 4.2MB/张×百张、encode 侧每次 7 倍像素缩放、pty 传输爆炸),且 384 的
  画质权衡(全屏微糊 vs 512)**留给用户配置**:等 user-config sub02 声明式接线
  (本地 spec 2026-06-04-user-config-sub02-declarative-wiring.md)落地后,把
  `COVER_MAX_DIM` 与 `COVER_STORAGE` 暴露为配置项,"用户存什么就放什么"。
  在代码注释里已有"配置系统接入后改读配置"的钩子注释,保持该口径。

## 4. 测试计划(TDD,先写测试)

| 测试 | 断言 |
|---|---|
| clamped 标记(单测,纯函数) | 1024² png → `clamped == true`;300² → `false`;尺寸仍 ≤384 |
| 自愈回写(tokio 测试,真 CacheIndex,参照现有 `cached_read_hits_disk` 的临时目录模式) | 预放 1024² png 进缓存 → 走 Remote 命中路径 → yield 若干次等 spawn 完成 → 重读缓存文件:字节已变为可解码的 ≤384 JPEG;再命中一次不再触发回写(clamped=false) |
| 取色行为回归 | 384² banded 大图:色板非空 + Lab 明度严格升序(沿用 `orders_swatches_by_ascending_lightness` 的构图法放大);同图调两次结果完全相等(确定性) |
| 现有测试 | `bytes_for_cache_*` / `large_image_is_clamped` / `small_image_unchanged` / cover_colors 全部保持绿 |

注意工程约定:测试无 unwrap/expect/索引,返回 `color_eyre::Result<()>`;
e2e 临时目录用 PID+纳秒后缀;文件 ≤800 行(hook 强制)。

## 5. 验证与预期

1. `cargo t` / `cargo td` / `cargo clippy --workspace --all-targets -- -D warnings` / `cargo fmt --check`;
2. 用户真机 + 重跑 `scripts/profile.sh`,预期:
   - 启动突刺 1.7 核×6s → **~0.2s wall**(单张 45ms→~5ms,4 fetch worker + blocking 池摊);
   - `resize_exact` / `extract_palette` / zune_jpeg 从 perf top 消失或降到 ~1%;
   - 首次跑会有一次性自愈回写(旧条目升级),第二次采样才是干净基线;
3. 按用户规矩:真机验证通过并明确同意后才 commit。

## 6. 关键 file:line 速查

- `crates/mineral-tui/src/runtime/cover_fetch.rs`:43 `COVER_MAX_DIM` / :63 `COVER_STORAGE` / :248 `fetch_and_decode` 命中分支 / :360 `decode_resize` / :384 `pack_blocking` / :418 `store_best_effort` / :465 `bytes_for_cache`
- `crates/mineral-tui/src/runtime/cover_colors.rs`:49 `extract_palette`
- `crates/mineral-tui/src/runtime/prefetch.rs`:21 `RADIUS`
- `crates/mineral-persist/src/cache_index.rs`:277 `put_bytes`(同 key 原地覆盖)

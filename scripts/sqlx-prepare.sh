#!/usr/bin/env bash
#
# ============================================================================
# mineral sqlx offline 缓存重建(改 SQL schema / query! 后跑)
# ============================================================================
#
# 干什么
# ------
# mineral 的 db crate(mineral-stats / mineral-persist)用 sqlx 的 `query!` 宏做
# 编译期 SQL 校验。校验数据缓存在仓库根的 `.sqlx/`(提交进 git),普通开发者与 CI
# 靠 `SQLX_OFFLINE=true` 读缓存离线编译,不需要任何真库。
#
# 只有**改了 migration / SQL / 新增 query!** 的人才需要重新生成缓存 —— 那正是本脚本
# 干的事:建一个一次性临时库、跑全部 migration 到最新、让 sqlx 重新 prepare。
#
# 为什么用临时库而非你实际在用的 stats.db
# ----------------------------------------
# sqlx 的 Migrator 在库的 `_sqlx_migrations` 表里按内容 checksum 记账。开发期反复改
# 同一条 migration 会让 checksum 变动;若拿实际库 prepare,实际库就记下了开发中途的
# 旧 checksum,之后 daemon 启动 `MIGRATOR.run()` 校验 mismatch → 拒绝启动 → 只能
# stats reset 丢历史。临时库用完即弃,没有这个账本负担;实际库全程零触碰,等代码合入
# 你正常启动时才第一次干净应用新 migration。
#
# 前置条件
# --------
#   1. 装 sqlx-cli:  cargo install sqlx-cli --no-default-features --features sqlite
#   2. 无需手建 .env —— 本脚本自带临时库 DATABASE_URL。若你要在编辑器里获得 rust-analyzer
#      的 sqlx 补全,可另建 gitignore 的根 `.env`:
#          DATABASE_URL=sqlite:scratch/stats-dev.db
#      (先跑一次本脚本把库建出来)。
#
# 用法
# ----
#   scripts/sqlx-prepare.sh          # 重建 .sqlx 缓存
#   git add .sqlx && git commit ...  # 缓存变动随代码一起提交
# ============================================================================

set -euo pipefail

cd "$(dirname "$0")/.."

# 临时库落 scratch/(已 gitignore、仓库内、人人可重建);相对路径对任何 clone 都成立。
mkdir -p scratch
DB_PATH="scratch/stats-dev.db"
export DATABASE_URL="sqlite:${DB_PATH}"

# 每次从零重建,避免残留 schema / 旧 checksum 干扰。
rm -f "$DB_PATH"
sqlx database create

# 跑到最新:每个 db crate 各自的 migrations 目录。新增 crate 时往这里加一行。
sqlx migrate run --source crates/mineral-stats/migrations

# 重新生成 .sqlx/ 缓存(--workspace:覆盖全 workspace 的 query!)。
cargo sqlx prepare --workspace

echo
echo "✓ .sqlx 缓存已重建。检查 git diff --stat .sqlx/ 后随代码提交。"

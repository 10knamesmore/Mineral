# 贡献指南

## 分支

- **`main`** — 已发布的稳定版,只读。**别对着 `main` 开 PR / 提交。**
- **`dev`** — 集成主干。所有改动从 `dev` 来、回 `dev` 去。
- **`feat/*` `fix/*`** — 你的 topic 分支,起于 `dev`。

## 提一个改动

1. 从最新 `dev` 切分支:
   ```bash
   git switch dev && git pull
   git switch -c feat/你的特性      # 或 fix/你修的问题
   ```
2. 干活。提交遵循 [Conventional Commits](https://www.conventionalcommits.org):
   `feat` / `fix` / `perf` / `refactor` / `test` / `chore` / `docs` / `ci`,带 scope,
   例 `feat(spectrum): …`
3. 本地过门禁(CI 会再跑一遍):
   ```bash
   cargo fmt --all --check
   cargo clippy --workspace --all-targets -- -D warnings
   cargo t          # = cargo nextest run --workspace
   ```
4. **开 PR 到 `dev`(不是 `main`)。** 一个 PR = 一个完整 topic;合并走 **Squash**,
   所以把 PR 标题写成这个 topic 的 commit(`feat(xxx): …`)。

> `feat` / `fix` 的 commit 会进 CHANGELOG(自动生成)。所以**用户能感知的改动就打
> `feat` / `fix`**(别打 `refactor`),PR 标题写得像一句 changelog 条目。

#!/usr/bin/env bash
#
# ============================================================================
# mineral 性能剖析脚本(perf + 火焰图素材)
# ============================================================================
#
# 干什么
# ------
# 用 perf 同时采样 mineral 的两个进程 —— TUI 前端 + daemon 后端 —— 覆盖真实的
# socket IPC 路径。产出 tui-perf.data / daemon-perf.data,交给人或 agent 用
# `perf report -i <file>` 离线分析(火焰图 / 调用树 / 热点)。
#
# 前置条件(一次性)
# ----------------
#   1. 装 perf：           sudo pacman -S perf          (或发行版的 linux-tools)
#   2. 放开采样权限：       sudo sysctl kernel.perf_event_paranoid=1
#   3. 放大 mmap 配额：     sudo sysctl kernel.perf_event_mlock_kb=2048
#      (两个 perf 并发各开 ring buffer,默认配额不够会报 "mmap failed" /
#       "Permission error mapping pages"。本脚本已用 `-m 128` 收着开,2048 足够。)
#   永久化：把 2/3 写进 /etc/sysctl.d/99-perf.conf
#
# 为什么是 perf 而不是 samply
# ---------------------------
# samply 的 attach 模式(`-p`)对运行中的多线程进程会 **ptrace-stop 目标**(把 daemon
# 挂起,采完才恢复;中途失败还会让 daemon 卡在 stopped),且逐线程开 mmap 容易撞配额。
# perf 用 perf_event 采样,**不 ptrace、不挂起目标**,符号取自 profiling build(下方),
# 更适合采一个必须持续运行的后台 daemon。
#
# 为什么要专门的 profiling build
# ------------------------------
# 火焰图要符号 + 真实优化行为:debug build 性能不可参考,release 又 strip 了符号。
# Cargo.toml 的 [profile.profiling] = release 优化 + 完整 DWARF,产物在
# target/profiling/,与正式 `--release` 物理隔离(符号不进发布二进制)。
#
# 怎么用
# ------
#   scripts/profile.sh                 # build → 起 daemon → 起 TUI 采样
#   scripts/profile.sh -- <args>       # `--` 之后的参数透传给 mineral 二进制
#   退出 TUI 后,两份 .data 落在仓库根(已进 .gitignore)。
#   分析： perf report -i tui-perf.data --stdio | head
#          perf report -i daemon-perf.data --stdio | head
#   ⚠ 分析期间别重新 `cargo build`,会覆盖 target/profiling/mineral 导致符号对不上。
#
# 实现说明
# --------
# 为可靠采到 daemon:脚本**自己起一个独立 daemon**、等 ready、perf attach 就位,再用
# perf 启动 TUI(`--connect` 连这个现成 daemon)。daemon pid 在 TUI 启动前就经
# `mineral status` 拿到(协议里有 pid 字段),不靠 pgrep 猜、不靠后台轮询碰时机。
# profile 完连带关掉 daemon,不留残留。对 profile 而言 TUI 行为 / IPC 路径与日常
# Auto 模式一致。两个子进程的 stdout/err 直接忽略(只关心 .data)。
# ============================================================================
set -euo pipefail
cd "$(dirname "$0")/.."

command -v perf >/dev/null 2>&1 || { echo "需要 perf：sudo pacman -S perf" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --) shift; break ;;
        *) break ;;
    esac
done

cargo build --profile profiling -p mineral
bin=./target/profiling/mineral

# DWARF 调用栈(profiling build 不保证 frame pointer);-F 999 控制采样频率;
# -m 128 显式定 ring buffer(512KB/cpu)—— 两个 perf 并发时不设 -m 会各取爆炸默认值、
# 抢爆 mmap 配额导致第二个 "Permission error mapping pages"。
perf_opts=(record -g --call-graph dwarf -F 999 -m 128)

cleanup() {
    [[ -n "${perf_daemon:-}" ]] && kill -INT "$perf_daemon" 2>/dev/null || true
    [[ -n "${perf_daemon:-}" ]] && wait "$perf_daemon" 2>/dev/null || true
    [[ -n "${serve_pid:-}" ]] && kill "$serve_pid" 2>/dev/null || true
    [[ -n "${serve_pid:-}" ]] && wait "$serve_pid" 2>/dev/null || true
}
trap cleanup EXIT

# 1. 脚本自起独立 daemon(out/err 忽略)。
"$bin" serve >/dev/null 2>&1 &
serve_pid=$!

# 2. 等 daemon ready(socket 可连)。
ready=0
for _ in $(seq 1 100); do
    if "$bin" status >/dev/null 2>&1; then ready=1; break; fi
    sleep 0.1
done
if [[ "$ready" -ne 1 ]]; then
    echo "daemon 10s 内未就绪。手动跑 \`$bin serve\` 看报错(build_channels 失败 / socket 权限等)。" >&2
    exit 1
fi

# 3. 拿 daemon pid，后台 perf attach(out/err 忽略;先就位,留 0.3s 让它 mmap 完成)。
dpid=$("$bin" status | awk '/^pid:/{print $2}')
echo "daemon pid=$dpid，attach 采样中…"
perf "${perf_opts[@]}" -o daemon-perf.data -p "$dpid" >/dev/null 2>&1 &
perf_daemon=$!
sleep 0.3

# 4. 前台 perf 启动 TUI(--connect 连现成 daemon)，采到退出。
perf "${perf_opts[@]}" -o tui-perf.data -- "$bin" --connect "$@" || true

# 5. 收尾(停 daemon 采样 + 关 daemon)。
cleanup
trap - EXIT

echo ""
echo "采样完成。产物："
ls -lh tui-perf.data daemon-perf.data 2>/dev/null || true
echo "分析： perf report -i tui-perf.data / daemon-perf.data"

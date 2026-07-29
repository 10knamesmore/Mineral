#!/usr/bin/env python3
"""检查 Codex 修改后的 Rust 文件是否符合仓库行数约束。

函数级约束由 clippy::too_many_lines 处理，此处只检查非测试模块的文件行数。
"""

import json
import sys
from pathlib import Path

FILE_HARD = 800
FILE_WARN = 500
PATCH_PATH_PREFIXES = ("*** Add File: ", "*** Update File: ", "*** Move to: ")


def count_non_test_loc(text: str) -> int:
    """计算排除 `#[cfg(test)] mod` 块后的行数。

    Args:
        text: Rust 源文件内容。

    Returns:
        不属于测试模块的行数。
    """
    keep = 0
    in_test_mod = False
    depth = 0
    pending = False
    for raw in text.splitlines():
        stripped = raw.strip()
        if not in_test_mod and stripped == "#[cfg(test)]":
            pending = True
            continue
        if pending:
            if stripped.startswith("mod ") and "{" in stripped:
                in_test_mod = True
                depth = stripped.count("{") - stripped.count("}")
                pending = False
                continue
            pending = False
        if in_test_mod:
            depth += stripped.count("{") - stripped.count("}")
            if depth <= 0:
                in_test_mod = False
            continue
        keep += 1
    return keep


def changed_paths(command: str, cwd: Path) -> list[Path]:
    """提取 Codex `apply_patch` 成功后可能存在的目标文件。

    Args:
        command: `tool_input.command` 中的原始 patch。
        cwd: Codex session 的工作目录。

    Returns:
        去重且保持 patch 顺序的目标路径。
    """
    paths = list[Path]()
    seen = set[Path]()
    for line in command.splitlines():
        raw_path = next(
            (
                line.removeprefix(prefix)
                for prefix in PATCH_PATH_PREFIXES
                if line.startswith(prefix)
            ),
            None,
        )
        if raw_path is None:
            continue
        path = Path(raw_path)
        resolved = path if path.is_absolute() else cwd / path
        if resolved not in seen:
            paths.append(resolved)
            seen.add(resolved)
    return paths


def check_file(path: Path) -> bool:
    """检查单个 Rust 文件并输出对应诊断。

    Args:
        path: 待检查的文件路径。

    Returns:
        文件是否未超过硬限制。
    """
    if path.suffix != ".rs" or not path.exists():
        return True
    parts = set(path.parts)
    if "tests" in parts or "target" in parts:
        return True

    loc = count_non_test_loc(path.read_text(encoding="utf-8", errors="replace"))
    if loc > FILE_HARD:
        sys.stderr.write(
            f"{path}: {loc} 行 > {FILE_HARD} 上限"
            "（不含 #[cfg(test)] mod），必须拆分。\n"
        )
        return False
    if loc > FILE_WARN:
        sys.stderr.write(f"{path}: {loc} 行，接近 {FILE_HARD} 上限。\n")
    return True


def main() -> int:
    """读取 Codex hook payload 并检查本次 patch 涉及的 Rust 文件。

    Returns:
        未超过硬限制时返回 0，否则返回 2。
    """
    data = json.load(sys.stdin)
    if data.get("tool_name") != "apply_patch":
        return 0

    tool_input = data.get("tool_input")
    if not isinstance(tool_input, dict):
        return 0
    command = tool_input.get("command")
    if not isinstance(command, str):
        return 0

    raw_cwd = data.get("cwd")
    cwd = Path(raw_cwd) if isinstance(raw_cwd, str) else Path.cwd()
    passed = all(check_file(path) for path in changed_paths(command, cwd))
    return 0 if passed else 2


if __name__ == "__main__":
    sys.exit(main())

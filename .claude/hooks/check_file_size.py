#!/usr/bin/env python3
"""文件级行数约束 hook (PostToolUse on Edit/Write/MultiEdit).

函数级约束由 clippy::too_many_lines 处理, 不在此重复实现.
"""
import json
import sys
from pathlib import Path

FILE_HARD = 800
FILE_WARN = 500


def count_non_test_loc(text: str) -> int:
    """剔除 #[cfg(test)] mod ... { ... } 块后的行数. 简单大括号配平."""
    keep = 0
    in_test_mod = False
    depth = 0
    pending = False
    for raw in text.splitlines():
        s = raw.strip()
        if not in_test_mod and s == "#[cfg(test)]":
            pending = True
            continue
        if pending:
            if s.startswith("mod ") and "{" in s:
                in_test_mod = True
                depth = s.count("{") - s.count("}")
                pending = False
                continue
            pending = False  # cfg(test) 是给 fn / use, 不是给 mod 的
        if in_test_mod:
            depth += s.count("{") - s.count("}")
            if depth <= 0:
                in_test_mod = False
            continue
        keep += 1
    return keep


def main() -> None:
    data = json.load(sys.stdin)
    if data.get("tool_name") not in ("Edit", "Write", "MultiEdit"):
        return
    path = data.get("tool_input", {}).get("file_path", "")
    if not path.endswith(".rs"):
        return
    p = Path(path)
    if not p.exists():
        return
    parts = set(p.parts)
    if "tests" in parts or "target" in parts:
        return  # 集成测试目录跳过

    loc = count_non_test_loc(p.read_text(encoding="utf-8", errors="replace"))
    if loc > FILE_HARD:
        sys.stderr.write(
            f"{path}: {loc} 行 > {FILE_HARD} 上限 (不含 #[cfg(test)] mod), 必须拆分.\n"
        )
        sys.exit(2)
    if loc > FILE_WARN:
        sys.stderr.write(f"{path}: {loc} 行, 接近 {FILE_HARD} 上限.\n")


if __name__ == "__main__":
    main()

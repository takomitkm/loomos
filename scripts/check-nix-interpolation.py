#!/usr/bin/env python3
"""本地 NiX 插值静态检查：扫描 '' 块内未转义的 ${...}，验证其是否在作用域声明。

用途：防「'' 块内注释/代码里写了未绑定插值」这类 NiX 求值期报错（多轮 CI
实证踩坑）。启发式检查，不替代 `nix flake check`，但能提前发现此类回归。

用法: python3 scripts/check-nix-interpolation.py [路径...]（默认 nix/ + flake.nix）
"""
import re
import sys
import pathlib

# 系统变量（mkDerivation 环境，始终可用）
SYS_VARS = {"out", "system", "name"}


def declared_vars(text: str) -> set:
    """收集文件作用域声明的顶层变量（函数参数 + let 块内行首绑定）。"""
    declared = set()
    # 函数参数块 { lib, stdenv, ... }:
    for m in re.finditer(r"\{([^{}]*)\}\s*:", text, re.DOTALL):
        for v in re.findall(r"[a-zA-Z_][a-zA-Z0-9_-]*", m.group(1)):
            declared.add(v)
    # let 块内的行首绑定（let 可嵌套，直接扫描所有 let...in 间的内容；
    # 简化处理：收集文件内所有行首缩进绑定的变量名，覆盖多层 let）
    for block in re.finditer(r"\blet\b(.*?)\b\nin\b", text, re.DOTALL):
        for m in re.finditer(r"^[ \t]+([a-zA-Z_][a-zA-Z0-9_-]*)[ \t]*=", block.group(1), re.M):
            declared.add(m.group(1))
    return declared | SYS_VARS


def check_block(text: str, path: pathlib.Path) -> list:
    """返回 [(行号, 表达式, 原因)] 列表。"""
    declared = declared_vars(text)
    problems = []
    for block in re.finditer(r"''(.*?)''", text, re.DOTALL):
        body = block.group(1)
        i = 0
        while i < len(body):
            if body[i] == "$" and i + 1 < len(body) and body[i + 1] == "{":
                prev = body[i - 1] if i > 0 else ""
                j = body.find("}", i + 2)
                if j == -1:
                    i += 1
                    continue
                expr = body[i + 2:j]
                # 转义形式 \$ 是合法 NiX 转义，不触发插值
                if prev == "\\":
                    i = j + 1
                    continue
                # 合法形态：属性访问（pkgs.foo）、函数调用（含括号）、lib/map 等
                if " " in expr or "(" in expr or expr.startswith(".") or expr in SYS_VARS:
                    i = j + 1
                    continue
                head = expr.split(".")[0].split("[")[0]
                line = text[: block.start()].count("\n") + body[:i].count("\n") + 2
                if head not in declared:
                    problems.append((line, expr, "未绑定"))
                i = j + 1
            else:
                i += 1
    return problems


def main() -> int:
    paths = [pathlib.Path(p) for p in sys.argv[1:]] or (
        sorted(pathlib.Path("nix").rglob("*.nix")) + [pathlib.Path("flake.nix")]
    )
    total = 0
    for p in paths:
        if not p.exists():
            continue
        text = p.read_text(encoding="utf-8")
        for line, expr, reason in check_block(text, p):
            total += 1
            print(f"{p}:{line} 插值 ${{{expr}}} → {reason}")
    print(f"\n检查结果: {total} 处未绑定插值（启发式，误报需人工确认）")
    return 1 if total else 0


if __name__ == "__main__":
    sys.exit(main())

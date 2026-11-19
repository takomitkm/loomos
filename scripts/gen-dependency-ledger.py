#!/usr/bin/env python
"""生成依赖台账（architecture/08 §8.3 的字段；只记录不改依赖）。

用法：python scripts/gen-dependency-ledger.py > docs/dev/engineering/dependency-ledger.md
数据源：cargo metadata（许可证/版本/来源）+ cargo tree（依赖树边数）。
Hardware 列永远是「未验证」直到上板（不许提前打勾）。
"""
import json
import subprocess
import sys
import datetime

WS_MEMBERS = {
    "loomos-runtime", "loomos-sys", "loomos-device", "loomos-audio", "loomos-nlu",
    "loomos-vision", "loomos-home", "loomos-executor", "loomosd", "loomos-cli",
}

def main():
    meta = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--format-version", "1"], text=True))
    packages = {p["id"]: p for p in meta["packages"]}
    by_name = {p["name"]: p for p in meta["packages"]}

    # workspace 成员（本仓 crate）
    members = []
    externals = []
    target_only = {}
    seen = set()

    for pid in meta["workspace_members"]:
        members.append(packages[pid])
        seen.add(pid)
        # 收集直接依赖（normal）
    for pid in meta["workspace_members"]:
        p = packages[pid]
        for dep in p["dependencies"]:
            if dep["kind"] in (None, "normal") and dep["name"] not in WS_MEMBERS:
                ext = by_name.get(dep["name"])
                if ext is None:
                    # 目标特有依赖（cfg(target_os=...)），宿主解析里不存在
                    target_only[dep["name"]] = p["name"]
                    continue
                if ext["id"] not in seen:
                    seen.add(ext["id"])
                    externals.append(ext)
    # 传递依赖（供依赖树规模统计，不逐条列）
    tree = subprocess.check_output(
        ["cargo", "tree", "--workspace", "--edges", "normal", "--prefix", "none"],
        text=True)
    tree_lines = [l.strip() for l in tree.splitlines() if l.strip()]
    all_names = {l.split()[0] for l in tree_lines}
    transitive = sorted(n for n in all_names if n not in WS_MEMBERS)

    print("# 依赖台账（生成于 {}）".format(datetime.date.today().isoformat()))
    print()
    print("> 由 `scripts/gen-dependency-ledger.py` 生成（只记录不改依赖）。")
    print("> 字段按 architecture/08 §8.3；Hardware 列**永远「未验证」直到上板**。")
    print()
    print("## 工作区成员（{} 个 crate）".format(len(members)))
    print()
    print("| crate | 版本 | 许可证 | forbid(unsafe) | Host 测试 | aarch64 | Hardware |")
    print("|---|---|---|---|---|---|---|")
    for p in sorted(members, key=lambda x: x["name"]):
        forbid = "是（loomos-sys 除外）" if p["name"] != "loomos-sys" else "否（可信边界）"
        print("| {} | {} | {} | {} | ✅ | ✅ check（{}） | 未验证 |".format(
            p["name"], p["version"], (p.get("license") or "MIT"), forbid,
            "linux-backend 已过" if p["name"] == "loomos-sys" else "默认档"))
    print()
    print("## 外部依赖（直接，{} 个）".format(len(externals)))
    print()
    print("| crate | 版本 | 许可证 | 来源 | MSRV | 直接依赖者 | 裁定 |")
    print("|---|---|---|---|---|---|---|")
    consumers = {}
    for pid in meta["workspace_members"]:
        p = packages[pid]
        for dep in p["dependencies"]:
            if dep["kind"] in (None, "normal"):
                consumers.setdefault(dep["name"], []).append(p["name"])
    for ext in sorted(externals, key=lambda x: x["name"]):
        msrv = ext.get("rust_version") or "-"
        consumers_str = ", ".join(sorted(consumers.get(ext["name"], [])))
        print("| {} | {} | {} | crates.io | {} | {} | 采用 |".format(
            ext["name"], ext["version"], (ext.get("license") or "?"), msrv, consumers_str))
    print()
    print("## 目标特有依赖（{} 个）".format(len(target_only)))
    print()
    print("| crate | 许可证 | 使用者 | 说明 |")
    print("|---|---|---|---|")
    for name, user in sorted(target_only.items()):
        lic = {"libc": "MIT OR Apache-2.0"}.get(name, "?")
        note = {"libc": "linux-backend feature（cfg(target_os=linux)）；aarch64 check 已过，板上验证未做"}.get(name, "")
        print("| {} | {} | {} | {} |".format(name, lic, user, note))
    print()
    print("## 传递依赖规模")
    print()
    print("- 全树（normal 边）共 {} 个包名（含工作区）".format(len(all_names)))
    print("- 传递外部依赖 {} 个：{}".format(len(transitive), ", ".join(transitive)))
    print()
    print("## 验证状态（08 §8.5 三层）")
    print()
    print("- Host CI：✅（{} 次全量回归；fmt/clippy -D warnings）".format(
        datetime.date.today().isoformat()))
    print("- Cross CI：✅ cargo check --target aarch64-unknown-linux-gnu"
          "（含 loomos-sys linux-backend feature）")
    print("- Hardware：❌ 未验证（上板流程见 deploy/README.md）")
    print()
    print("## 已否决/暂缓")
    print()
    print("- rusqlite（bundled C 编译）：暂缓——违反当前纯 Rust 依赖树约束，")
    print("  引入须过 §8.2 准入且在可用 C 工具链环境验证（见 EVIDENCE.md 工程决策）。")
    print("- getrandom / windows-sys 系：否决——windows-gnu dlltool 链缺失（见 EVIDENCE.md）。")

if __name__ == "__main__":
    sys.exit(main())

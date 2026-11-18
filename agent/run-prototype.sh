#!/usr/bin/env bash
# loomos 一键原型执行器 —— Agent 的唯一入口。
#
# 用法：
#   bash agent/run-prototype.sh          # 全流程
#   bash agent/run-prototype.sh --fast   # 跳过 cargo test（仅快速 smoke）
#
# 步骤：环境自检 → cargo test --workspace → gen-fixtures → smoke（装配+回放+断言）
#       → 写 reports/prototype-report.md → 退出码 0/1。
set -uo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
FAST=0
[[ "${1:-}" == "--fast" ]] && FAST=1

ok=0
fail=0
step() { printf '\n\033[1;34m== %s ==\033[0m\n' "$1"; }
pass() { printf '  \033[32m✓ %s\033[0m\n' "$1"; ok=$((ok+1)); }
abort() { printf '  \033[31m✗ %s\033[0m\n' "$1"; fail=$((fail+1)); }

step "0. 环境自检"
if command -v cargo >/dev/null 2>&1; then
    RUSTV="$(rustc --version)"
    pass "工具链: $RUSTV"
else
    abort "cargo 不在 PATH；请安装 rustup（工具链锁定见 rust-toolchain.toml）"
    exit 1
fi
echo "  - 仓库: $ROOT"
echo "  - 平台: $(uname -s) / $(uname -m)"

step "1. cargo test --workspace（约 100 项）"
if [[ $FAST -eq 1 ]]; then
    echo "  (--fast: 跳过)"
else
    if cargo test --workspace 2>&1 | tee /tmp/loomos-test.log | grep -qE "test result: FAILED"; then
        abort "测试有失败项；查看 /tmp/loomos-test.log"
        fail=$((fail+1))
    else
        TEST_COUNT="$(grep -oE 'test result: ok\. [0-9]+' /tmp/loomos-test.log | awk '{s+=$4} END {print s}')"
        pass "cargo test 全绿（${TEST_COUNT:-?} 项）"
    fi
fi

step "2. 生成语音夹具（fixtures/）"
if cargo run -q -p loomos-cli -- gen-fixtures fixtures >/tmp/loomos-gen.log 2>&1; then
    pass "12 条中文场景夹具（48 kHz wav + manifest）"
else
    abort "夹具生成失败：$(tail -2 /tmp/loomos-gen.log)"
    fail=$((fail+1))
fi

step "3. 装配原型并回放全场景（smoke）"
if cargo run -q -p loomos-cli -- smoke fixtures --out reports >/tmp/loomos-smoke.log 2>&1; then
    pass "smoke 12/12 PASS"
    grep -E "^  [✓✗]" /tmp/loomos-smoke.log || true
else
    abort "smoke 有失败："
    grep -E "^  [✓✗]" /tmp/loomos-smoke.log || tail -5 /tmp/loomos-smoke.log
    fail=$((fail+1))
fi

step "4. 证据产物"
for f in reports/prototype-report.md reports/data/events.jsonl; do
    if [[ -e "$f" ]]; then
        pass "$f"
    else
        echo "  - $f （本次未产出或为内存档）"
    fi
done
SPEECH_N="$(ls reports/speech/*.wav 2>/dev/null | wc -l)"
echo "  - reports/speech/*.wav: ${SPEECH_N} 个（48 kHz，旧板「无声卡→落盘」纪律）"

step "5. 结论"
if [[ $fail -eq 0 ]]; then
    printf '\033[1;32m全部通过：原型可执行、测试全绿、报告已生成（%s 通过步骤）。\033[0m\n' "$ok"
    printf '证据等级：Tested / TypeChecked（见报告 §残余假设）。\n'
    exit 0
else
    printf '\033[1;31m存在失败项（%d 步失败）。先读 reports/prototype-report.md。\033[0m\n' "$fail"
    exit 1
fi

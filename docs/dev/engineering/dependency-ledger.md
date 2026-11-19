# 依赖台账（生成于 2026-11-20）

> 由 `scripts/gen-dependency-ledger.py` 生成（只记录不改依赖）。
> 字段按 architecture/08 §8.3；Hardware 列**永远「未验证」直到上板**。

## 工作区成员（14 个 crate）

| crate | 版本 | 许可证 | forbid(unsafe) | Host 测试 | aarch64 | Hardware |
|---|---|---|---|---|---|---|
| loomos-adapters-sherpa | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-audio | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-cli | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-device | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-executor | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-home | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-inference | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-nlu | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-nlu-bench | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-nlu-wasm-wrap | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-runtime | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomos-sys | 0.1.0 | MIT | 否（可信边界） | ✅ | ✅ check（linux-backend 已过） | 未验证 |
| loomos-vision | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |
| loomosd | 0.1.0 | MIT | 是（loomos-sys 除外） | ✅ | ✅ check（默认档） | 未验证 |

## 外部依赖（直接，5 个）

| crate | 版本 | 许可证 | 来源 | MSRV | 直接依赖者 | 裁定 |
|---|---|---|---|---|---|---|
| serde | 1.0.229 | MIT OR Apache-2.0 | crates.io | 1.56 | loomos-adapters-sherpa, loomos-audio, loomos-cli, loomos-device, loomos-executor, loomos-home, loomos-inference, loomos-nlu, loomos-runtime, loomos-vision, loomosd | 采用 |
| serde_json | 1.0.154 | MIT OR Apache-2.0 | crates.io | 1.71 | loomos-adapters-sherpa, loomos-audio, loomos-cli, loomos-device, loomos-executor, loomos-home, loomos-inference, loomos-nlu, loomos-nlu-bench, loomos-nlu-wasm-wrap, loomos-runtime, loomos-vision, loomosd | 采用 |
| thiserror | 2.0.21 | MIT OR Apache-2.0 | crates.io | 1.77 | loomos-adapters-sherpa, loomos-audio, loomos-cli, loomos-device, loomos-executor, loomos-home, loomos-inference, loomos-nlu, loomos-runtime, loomos-vision, loomosd | 采用 |
| tracing | 0.1.44 | MIT | crates.io | 1.65.0 | loomos-adapters-sherpa, loomos-audio, loomos-cli, loomos-device, loomos-executor, loomos-home, loomos-inference, loomos-nlu, loomos-runtime, loomos-vision, loomosd | 采用 |
| tracing-subscriber | 0.3.23 | MIT | crates.io | 1.65.0 | loomos-cli, loomosd | 采用 |

## 目标特有依赖（1 个）

| crate | 许可证 | 使用者 | 说明 |
|---|---|---|---|
| libc | MIT OR Apache-2.0 | loomos-sys | linux-backend feature（cfg(target_os=linux)）；aarch64 check 已过，板上验证未做 |

## 传递依赖规模

- 全树（normal 边）共 40 个包名（含工作区）
- 传递外部依赖 30 个：cfg-if, itoa, lazy_static, loomos-adapters-sherpa, loomos-inference, loomos-nlu-bench, loomos-nlu-wasm-wrap, matchers, memchr, once_cell, pin-project-lite, proc-macro2, quote, regex-automata, regex-syntax, serde, serde_core, serde_derive, serde_json, sharded-slab, syn, thiserror, thiserror-impl, thread_local, tracing, tracing-attributes, tracing-core, tracing-subscriber, unicode-ident, zmij

## 验证状态（08 §8.5 三层）

- Host CI：✅（2026-11-20 次全量回归；fmt/clippy -D warnings）
- Cross CI：✅ cargo check --target aarch64-unknown-linux-gnu（含 loomos-sys linux-backend feature）
- Hardware：❌ 未验证（上板流程见 deploy/README.md）

## 已否决/暂缓

- rusqlite（bundled C 编译）：暂缓——违反当前纯 Rust 依赖树约束，
  引入须过 §8.2 准入且在可用 C 工具链环境验证（见 EVIDENCE.md 工程决策）。
- getrandom / windows-sys 系：否决——windows-gnu dlltool 链缺失（见 EVIDENCE.md）。

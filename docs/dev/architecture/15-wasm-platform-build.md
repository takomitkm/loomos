# 15 — Wasm 平台构建

> **层级**：`architecture/` 跨机型现行版（编号顺延）。裁定依据：`engineering/ADR-011 v1.1`。
> 三形态基准证据见 `loomos-nlu-bench`（native/interp/AOT checksum 全一致）。

## 15.1 构建流水线

```
Rust 组件源码（loomos-nlu / -home / -executor / ...）
        |  Nix 钉死工具链（rust 1.99.1，= rust-toolchain.toml）
        v
wasm32-wasip1（cdylib，业务代码零改动）
        v
正本 .wasm（loomos-components 包产物）
        |                            |
        v                            v
Native WAMR Host 构建           wamrc（build 机运行）
（按 OS/ISA 分目标）             （--target 决定产物 ISA——勿混淆 build/target）
        |                            |
        +------------+---------------+
                     v
             平台发布（.wasm + .aot + AOT 兼容键 sidecar）
                     v
             校验（checksum + 兼容键）→ 签名工件
```

## 15.2 WAMR 构建矩阵

| 目标 | 产物 | 构建方式 |
|---|---|---|
| Linux x86_64 | libiwasm.a + iwasm | nix `.#loomos-wamr`（AOT+INTERP+WASI P1 最小集） |
| Linux aarch64 | libiwasm.a | nix 交叉 / aarch64 builder |
| Windows x64 | iwasm.lib | 原生 runner（P4；非 nix 职责——nix 不管理 Windows OS） |
| macOS x64/ARM64 | libiwasm.a | 原生 runner（P4） |

wamrc 一份（build 机）可为全部 ISA 产出 .aot（`--target=aarch64` 等）；
Windows 交叉参数见 nlu-bench 实测：`--target=x86_64-pc-windows-msvc --target-abi=msvc --size-level=1`。

## 15.3 AOT 兼容键（加载前强制校验）

`.aot` 绝不因「架构名看起来兼容」而加载。兼容键字段（sidecar `module.aot.compat.json`）：
WAMR 版本 / 构建配置 / wamrc 版本 / OS / ISA / CPU 基线 / ABI / Host ABI 版本 /
wasm 源哈希 / 编译旗标。

- 校验层次：① loomis-wamr aot_compat（来源/管线追溯）→ ② WAMR 本体 aot_load（ISA/ABI 权威执行点）。
- 生产档：sidecar 缺失/不匹配 → **fail closed**；开发档：缺失放行（tracing::warn）。
- 魔数/版本实证：`\0aot` + 格式版本 6（WAMR core/config.h @06f6c8c）。

## 15.4 WAMR 实测语义（loomos-wamr/loomos-wamr-sys 实现依据）

- `call_wasm` 返回值在 **argv[0]**（非 argv[argc]）；
- 模块缓冲与模块同生命周期（WAMR 可能引用其内容）；
- 静态库名 libiwasm.a（vmlib target OUTPUT_NAME=iwasm）；
- Windows AOT 加载需 MAP_32BIT 生效（高地址 VirtualAlloc → ADDR32 重定位失败，
  本机补丁在 nlu-bench/dl 的 WAMR 克隆）；
- RuntimeInitArgs.mem_alloc_option 联合体 32 字节（bench 的 [u8;24] 靠零填充侥幸，
  生产实现已按 wasm_export.h @06f6c8c 修正）。

## 15.5 生命周期与预算契约（loomos-wamr P1 已交付）

- RuntimeHost 进程级单例；关闭顺序：停调用 → 反实例化全部 → 卸载全部模块 →
  释放预算 → 销毁运行时；Drop 仅兜底并大声记录（不假设 Drop 保证清理）。
- 内存预算：全局 + 单实例，**有界**（0=不限拒绝）；CAS 原子预留 → 提交/回滚；
  并发准入测试（8 线程，累计永不越预算）。Guest 线性内存预算 ≠ 进程 RSS（勿混谈）。

## 15.6 Host ABI（P2 已交付，接入在 P3）

- 通道：Core Wasm + WASI P1 + `loomos` 命名空间 11 接口（`loomos.<iface>_v1`）；
- 每调用：调用方身份（线程级 ctx）→ CapabilityChecker（权威在 loomos-runtime
  CapabilityStore）→ Guest 内存 validate（sys 层复制语义，native 指针不外泄）→
  负载限额（64KiB，大块走 buffer 句柄）→ 属主句柄表（不透明 u64）；
- 稳定错误码即 ABI（INVALID/CAPABILITY/MEMORY/PAYLOAD/HANDLE/UNSUPPORTED/BUSY/UNLINKED）；
- 控制面（小消息）走 guest 内存拷贝；数据面（音频/图像/张量）走 buffer 句柄 +
  Host 持有的物理缓冲，**不做反复 JSON 序列化**。

## 15.7 迁移状态（P3 范围）

| 组件 | 状态 |
|---|---|
| loomis-nlu（经 loomos-nlu-wasm-wrap） | wasm32-wasip1 构建 ✓（bench 三形态验证）；Host ABI 化待 P3 |
| loomis-home / executor / audio / vision 业务逻辑 | Native，迁移计划见 ADR-011 §主权边界（业务逻辑可迁，硬件/数据面留 Native） |
| 权威组件（capability/lease/supervisor/lifecycle/config 权威） | **永不 Wasm 化**（归档 0001 否决记录） |

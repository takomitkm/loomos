---
title: "ADR-011: LoomOS 统一采用 Wasm AOT 执行组件"
version: 1.1
status: accepted
date: 2026-11-22
deciders: takomitkm
tags:
  - wasm
  - wamr
  - architecture-decision
  - component-model
  - nix
---

# ADR-011: LoomOS 统一采用 Wasm AOT 执行组件

## 状态

**Accepted**（v1.0 2026-11-22；**v1.1 2026-11-22 修订**：修正权限/资源主权边界、
Host ABI 命名、性能表述与命名拼写；修订记录见下节）

## 修订记录（v1.0 → v1.1）

| # | 修正 | 原因 |
|---|---|---|
| 1 | 架构图删除「loomos-runtime 可部署部分（config / capability / lifecycle）→ Wasm」 | 权威组件（CapabilityStore / ResourceLease / Supervisor / LifecycleManager / 配置权威）必须留在 Native Host；Guest 永不自授权。错误方案全文归档于 `docs/dev/archive/0001-superseded-wasm-authority-migration.md` |
| 2 | 新增 §主权边界 一节，显式列出 Native Host 不可 Wasm 化清单 | v1.0 只写「平台适配留 Native」，未覆盖权威组件，被误读为可迁移 |
| 3 | Host ABI 函数名统一为 `loomos.<iface>_v1`（如 `loomos.log_v1`） | v1.0 的 `loomos_config_v1` 分组命名与接口标识不一致 |
| 4 | 性能表述改为「仅已测负载成立」 | NLU 基准（loomos-nlu-bench）只证明该负载 AOT≈Native，不外推为所有 Wasm 工作负载 |
| 5 | 命名统一 `loomooms-*` → `loomos-*`；crate 目录 `crates/loomos-wamr`；新增独立可信边界 crate `loomos-wamr-sys` | 全仓命名一致性；raw WAMR FFI 集中到第三可信边界（与 loomos-sys / loomos-ort-sys 同地位），`loomos-wamr` 门面恢复 `#![forbid(unsafe_code)]` |

## 背景

LoomOS 原设计在 `ComponentManifest.execution` 中允许每个组件选择 Native / Wasm AOT /
Wasm Interpreter 执行方式。这导致：

1. **双套实现**：每个组件都可能需要 Native 和 Wasm 两条路径，维护成本倍增。
2. **执行后端选择泄漏到业务层**：Manifest 本应描述组件"是什么"，却被迫回答"怎么跑"。
3. **不可预测的信任边界**：Native 组件拥有全部系统能力，Wasm 组件受限——但界限由
   Manifest 决定而非系统架构保证。
4. **NLU 基准已证明 Wasm 可行**：loomos-nlu-bench 已验证 WAMR Interpreter / AOT
   能执行真实 loomos-nlu 业务代码；**在该负载上** AOT 性能 ≈ Native（P50 1.2µs vs
   400ns，本机读数，来源 EVIDENCE.md），checksum 全一致。该结论**不外推**为所有
   Wasm 工作负载的原生等价性能。

同时，WAMR 官方已支持 Linux（x86-64/AArch64）、Windows（x64）、macOS（ARM64/x86-64）、
Android、Zephyr、NuttX、RT-Thread 等平台；指令集覆盖 x86、ARM、AArch64、RISC-V；
Cortex-A53 测试支持已确认。

## 决定

1. **LoomOS 所有可部署组件统一编译为 WebAssembly**（`wasm32-wasip1` 目标）。
2. **生产执行模式统一为 WAMR AOT**（`wamrc` 预编译为目标平台 `.aot`）。
3. **开发后备：WAMR Interpreter**（无需 wamrc，直接跑 `.wasm`）。
4. **Native Host 保留**——负责 OS API / 设备 / NPU / DMA / 文件系统等平台适配，
   以及**全部权威组件**（见下节），但**不再作为业务组件的执行后端**。
5. **ComponentManifest 移除 `execution` 字段**——执行模型由系统统一规定，
   Manifest 只描述身份/依赖/权限/资源预算/生命周期。
6. **AI 推理后端留在 Native Host**，通过 Host ABI 以稳定接口暴露给 Wasm Guest
   （不允许 Guest 直接取得 DMA 地址或厂商 Runtime 指针）。
7. **Wasm 分发格式分层**：`.wasm` 为统一分发产物，`.aot` 为目标平台预编译缓存
   （由 Nix/CI 的 `wamrc` 自动产出，安装器按 AOT 兼容键自动选择）。

## 主权边界（v1.1 修正）

**统一的是组件执行格式，不是把操作系统权威搬进 Wasm。**

以下组件与职责**永远留在 Native Host**，不作为 Guest 代码部署：

| Native Host 主权组件 | 职责 | Guest 访问方式 |
|---|---|---|
| `loomos-runtime` CapabilityStore | 能力授权与运行时授权 | Host ABI 客户端（每次调用由 Host 校验） |
| `loomos-runtime` LeaseManager（ResourceLease） | 资源租约与所有权 | 只拿不透明句柄 |
| `loomos-runtime` Supervisor | 故障分级与恢复决策 | 无（被监管者） |
| `loomos-runtime` component 生命周期 | 权威生命周期状态机 | 无（被动状态） |
| Configuration Service（权威端） | Schema/Revision/Outbox | `loomos.config_get_v1` 只读 |
| 推理编排 + ONNX/sherpa/NPU Runtime | 硬件所有权、DMA、缓存同步 | `loomos.infer_submit_v1` 等 |
| WAMR Runtime 本体 | 执行 Wasm | —（宿主进程内嵌） |
| `loomos-sys` / `loomos-ort-sys` / `loomos-wamr-sys` | 三条声明式可信边界 | — |

分工总纲：

- **Wasm** 执行业务逻辑；
- **Native Host** 授权操作、持有资源与硬件；
- **Linux** 执行内核级强制；
- **Nix** 重建声明系统配置（见 ADR-012）。

这不是「双执行后端」——是**一种执行格式 + 一个特权宿主**。

## 架构

```
LoomOS
 ├── Native Platform Host（Rust，权威与平台层，不 Wasm 化）
 │     ├── 权威组件：CapabilityStore / ResourceLease / Supervisor / Lifecycle / 配置权威
 │     ├── Linux / Windows / macOS 平台适配
 │     ├── 文件系统 / 网络 / 硬件 / NPU / DMA
 │     ├── ONNX Runtime / sherpa-onnx / 厂商 NPU Runtime
 │     └── WAMR Runtime（宿主进程内嵌；FFI 经 loomos-wamr-sys）
 │
 └── Wasm Components（统一 wasm32-wasip1 编译目标）
       ├── loomos-nlu（自然语言理解）
       ├── loomos-home（意图→设备操作业务规则）
       ├── loomos-executor（业务动作编排）
       ├── loomos-audio 业务逻辑（VAD/KWS/ASR/TTS 档位调度）
       ├── loomos-vision 业务逻辑（YuNet/SFace 档位调度）
       └── 用户扩展组件
```

## Host ABI（P2）

Wasm Guest 通过版本化 Host ABI 与 Native Host 交互。不允许 Guest 直接取得
物理 DMA 地址或厂商 Runtime 指针。

```
Wasm Guest
    |
    | loomos.infer_submit_v1(model_id, buffer_handle)
    v
WAMR Host API（native symbol 注册，经 loomos-wamr-sys）
    |
    v
LoomOS Native Host
    +-- ONNX Runtime（通用推理）
    +-- sherpa-onnx（语音：VAD/KWS/ASR/TTS）
    +-- Vendor NPU Runtime（板端）
    +-- DMA / Cache Sync
```

### WASI Preview 1 + 自定义 Host API

第一版不使用 Component Model，采用 **Core Wasm + WASI P1 + 自定义 Host 函数**：

- **WASI P1 标准导入**：`fd_write`、`clock_time_get`、`random_get`、`fd_read` 等
- **自定义 `loomos` 命名空间（版本化接口标识，v1）**：

| 接口 | 说明 |
|---|---|
| `loomos.log_v1` | 结构化日志输出 |
| `loomos.time_mono_v1` | 单调时钟 |
| `loomos.time_wall_v1` | 墙钟 |
| `loomos.config_get_v1` | 配置只读（namespace + key → ConfigValue） |
| `loomos.infer_submit_v1` | 推理提交（model_id + buffer_handle） |
| `loomos.infer_poll_v1` | 推理结果轮询 |
| `loomos.device_op_v1` | 设备操作提交（device_id + op + params） |
| `loomos.device_poll_v1` | 设备操作结果轮询 |
| `loomos.buffer_import_v1` | 共享缓冲导入（不透明句柄） |
| `loomos.buffer_release_v1` | 缓冲所有权归还 |
| `loomos.event_subscribe_v1` | 事件订阅 |

接口标识不是权限授予——每次调用仍由 Native Host 校验该组件的能力。

### 为什么不是 Component Model

WAMR 对 Component Model 的支持尚在演进；Preview 1 已稳定且 WAMR 长期支持。
Component Model 的价值（跨语言类型、compose 语义）在 loomos 场景中
当前组件全是 Rust 编写、无跨语言需求，暂不需要。

未来如果需要跨语言组件互操作，可以升级到 Component Model——Host ABI 的
版本化设计为此预留了空间。

## 后果

### 正面

1. **单一实现**：每个组件只有一份业务代码（Rust → wasm32-wasip1）。
2. **天然沙箱**：Wasm 线性内存隔离 + 能力导入 = 安全边界由运行时保证。
3. **跨平台分发**：`.wasm` 一次编译到处部署，`.aot` 按 WAMR 平台矩阵覆盖。
4. **热更新**：Wasm 模块可运行时替换（WAMR 支持 unload + load）。
5. **简化 Manifest**：不再有执行后端选择字段。

### 负面

1. **性能差距**：**仅就已测负载**（NLU 基准）AOT ≈ Native；通用结论不成立，
   每类负载需各自基准。AOT ≈ Native 但非 100%（wasm call overhead ~几 ns/call）。
2. **Host ABI 维护**：版本化接口是长期承诺。
3. **NPU/DMA 不可直通**：Guest 必须通过 Host ABI 间接访问（额外一跳延迟）。
4. **调试复杂度**：Wasm 堆栈跟踪不如原生清晰。
5. **权威组件必须原生**：业务 Wasm 化不减少 Native 面积（主权边界一节）。

### 中性

1. **内存模型**：Wasm 线性内存（32 位地址空间）≠ Native 堆——数据跨界需要序列化
   或共享内存映射；大媒体缓冲走句柄+有界拷贝，不走 JSON 反复序列化。
2. **WASI P1 不覆盖全部 OS 能力**：自定义 Host 函数补齐（loomos 命名空间）。

## 替代方案考虑

| 方案 | 为什么不选 |
|---|---|
| 保持 Native/Wasm 双轨 | 双套实现维护成本高；Native/Wasm 界限由人决定而非系统保证 |
| 使用 wasmtime 而非 WAMR | wasmtime 面向服务端场景（内存占用大），WAMR 面向嵌入式（A53 验证过） |
| 完整 Component Model | WAMR 支持尚在演进；P1 已够；未来可升级 |
| 不用 Wasm，全部 Native | 失去沙箱隔离、热更新、跨平台分发三大核心价值 |
| 把 CapabilityStore/Supervisor 等权威组件迁入 Wasm | Guest 自授权悖论、租约与硬件事实脱节、被监管者不能自我恢复（见归档 0001） |

## 相关

- `docs/dev/archive/0001-superseded-wasm-authority-migration.md`：被否决的权威迁入 Wasm 方案（本 ADR v1.0 的错误边界）
- `docs/dev/engineering/ADR-012-Nix系统重建与包管理权威.md`：Nix 系统重建职责
- `docs/dev/engineering/ADR-013-AB系统镜像与回滚.md`：A/B 部署与回滚
- `docs/dev/architecture/14-nix-system-reconstruction.md` / `15-wasm-platform-build.md` / `16-system-image-and-ab.md`
- `docs/dev/engineering/03-OS代劳清单.md` §B：AI 后端留 Native Host 的映射
- `docs/dev/architecture/06-device-framework.md` §6.6：Domain Adapter 插槽
- `docs/dev/platforms/FMQL30TAI676H/07-platform-baseline.md`：板端 DMA/Cache 事实
- [WAMR 官方](https://github.com/bytecodealliance/wasm-micro-runtime)
- [WAMR core/config.h（AOT_MAGIC_NUMBER=0x746f6100, AOT_CURRENT_VERSION=6）](https://github.com/bytecodealliance/wasm-micro-runtime/blob/main/core/config.h)

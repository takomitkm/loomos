---
title: "ADR-011: LoomOS 统一采用 Wasm AOT 执行组件"
version: 1.0
status: accepted
date: 2026-11-22
deciders: takomitkm
tags:
  - wasm
  - wamr
  - architecture-decision
  - component-model
---

# ADR-011: LoomOS 统一采用 Wasm AOT 执行组件

## 状态

**Accepted**（2026-11-22）

## 背景

LoomOS 原设计在 `ComponentManifest.execution` 中允许每个组件选择 Native / Wasm AOT /
Wasm Interpreter 执行方式。这导致：

1. **双套实现**：每个组件都可能需要 Native 和 Wasm 两条路径，维护成本倍增。
2. **执行后端选择泄漏到业务层**：Manifest 本应描述组件"是什么"，却被迫回答"怎么跑"。
3. **不可预测的信任边界**：Native 组件拥有全部系统能力，Wasm 组件受限——但界限由
   Manifest 决定而非系统架构保证。
4. **NLU 基准已证明 Wasm 可行**：loomos-nlu-bench 已验证 WAMR Interpreter / AOT
   能执行真实 loomooms-nlu 业务代码，AOT 性能 ≈ Native，checksum 全一致。

同时，WAMR 官方已支持 Linux（x86-64/AArch64）、Windows（x64）、macOS（ARM64/x86-64）、
Android、Zephyr、NuttX、RT-Thread 等平台；指令集覆盖 x86、ARM、AArch64、RISC-V；
Cortex-A53 测试支持已确认。

## 决定

1. **LoomOS 所有可部署组件统一编译为 WebAssembly**（`wasm32-wasip1` 目标）。
2. **生产执行模式统一为 WAMR AOT**（`wamrc` 预编译为目标平台 `.aot`）。
3. **开发后备：WAMR Interpreter**（无需 wamrc，直接跑 `.wasm`）。
4. **Native Host 保留**——负责 OS API / 设备 / NPU / DMA / 文件系统等
   平台适配，但**不再作为组件执行后端**。
5. **ComponentManifest 移除 `execution` 字段**——执行模型由系统统一规定，
   Manifest 只描述身份/依赖/权限/资源预算/生命周期。
6. **AI 推理后端留在 Native Host**，通过 Host ABI 以稳定接口暴露给 Wasm Guest
   （不允许 Guest 直接取得 DMA 地址或厂商 Runtime 指针）。
7. **Wasm 分发格式分层**：`.wasm` 为统一分发产物，`.aot` 为目标平台预编译缓存
   （由 CI 的 `wamrc` 自动产出，安装器自动选择）。

## 架构

```
LoomOS
 ├── Native Platform Host（Rust，不可 Wasm 化的底层）
 │     ├── Linux / Windows / macOS 平台适配
 │     ├── 文件系统 / 网络 / 硬件 / NPU / DMA
 │     ├── ONNX Runtime / sherpa-onnx / 厂商 NPU Runtime
 │     └── WAMR Runtime（宿主进程内嵌）
 │
 └── Wasm Components（统一 wasm32-wasip1 编译目标）
       ├── loomooms-runtime 可部署部分（config / capability / lifecycle）
       ├── loomooms-nlu（自然语言理解）
       ├── loomooms-home（意图→设备操作）
       ├── loomooms-executor（双通道执行器）
       ├── loomooms-audio 业务逻辑（VAD/KWS/ASR/TTS 档位调度）
       ├── loomooms-vision 业务逻辑（YuNet/SFace 档位调度）
       └── Applications
```

**统一的是组件执行格式，不是要求整个操作系统没有 Native 代码。**

## Host ABI（P2）

Wasm Guest 通过版本化 Host ABI 与 Native Host 交互。不允许 Guest 直接取得
物理 DMA 地址或厂商 Runtime 指针。

```
Wasm Guest
    |
    | loomos.infer.submit(model_id, buffer_handle)
    v
WAMR Host API（native symbol 注册）
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
- **自定义 `loomos.*` Host 函数**（版本化命名空间）：
  - `loomos_config_v1`：配置读取（namespace + key → ConfigValue）
  - `loomos_log_v1`：结构化日志输出
  - `loomos_infer_v1`：推理提交（model_id + buffer_handle）
  - `loomos_device_v1`：设备操作提交（device_id + op + params）
  - `loomos_time_v1`：单调时钟 + 墙钟

### 为什么不是 Component Model

WAMR 对 Component Model 的支持尚在演进；Preview 1 已稳定且 WAMR 长期支持。
Component Model 的价值（跨语言类型、compose 语义）在 loomooms 场景中
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

1. **性能差距**：Wasm AOT ≈ Native 但非 100%（wasm call overhead ~几 ns/call）。
2. **Host ABI 维护**：版本化接口是长期承诺。
3. **NPU/DMA 不可直通**：Guest 必须通过 Host ABI 间接访问（额外一跳延迟）。
4. **调试复杂度**：Wasm 堆栈跟踪不如原生清晰。

### 中性

1. **内存模型**：Wasm 线性内存（32 位地址空间）≠ Native 堆——数据跨界需要序列化
   或共享内存映射。
2. **WASI P1 不覆盖全部 OS 能力**：自定义 Host 函数补齐（loomos.* 命名空间）。

## 替代方案考虑

| 方案 | 为什么不选 |
|---|---|
| 保持 Native/Wasm 双轨 | 双套实现维护成本高；Native/Wasm 界限由人决定而非系统保证 |
| 使用 wasmtime 而非 WAMR | wasmtime 面向服务端场景（内存占用大），WAMR 面向嵌入式（A53 验证过） |
| 完整 Component Model | WAMR 支持尚在演进；P1 已够；未来可升级 |
| 不用 Wasm，全部 Native | 失去沙箱隔离、热更新、跨平台分发三大核心价值 |

## 相关

- `docs/dev/engineering/03-OS代劳清单.md` §B：AI 后端留 Native Host 的映射
- `docs/dev/architecture/06-device-framework.md` §6.6：Domain Adapter 插槽
- `docs/dev/platforms/FMQL30TAI676H/07-platform-baseline.md`：板端 DMA/Cache 事实
- [WAMR 官方](https://github.com/bytecodealliance/wasm-micro-runtime)

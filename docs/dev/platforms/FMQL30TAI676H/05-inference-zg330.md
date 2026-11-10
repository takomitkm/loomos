# 05 — 异构推理：ZG330 / ICraft 本板落地

> **层级**：通用编排契约（数据流、Orchestration 职责、同步阻塞约束、FFI 边界、CPU Backend 评估口径、整模型优先原则）见 `../../architecture/05-inference-orchestration.md`。
> **本文只写 ICraft 3.39.0 + ZG330 的本板事实与判定**，不重复通用契约。

---

## 5.2 ICraft 能力边界（3.39.0）

`[FACT-A/B]`：

- CLI/板端 Runtime 版本 **3.39.0**（CLI build `3.39.0.0-431be90`）。
- 正式 API 包含 Session、Device、Tensor、Backend、MemRegion、MemManager 等。
- **当前调查未发现 ICraft 系统级 Scheduler。**
- **`forward` 同步阻塞**；未发现 cancel/abort API。
- Server 文档**不建议同时打开多个 Server**。
- `FForwardBackend` 中有 `priority` 参数但**语义未解释**，不能当作已验证的任务优先级接口。
- `Device::reset(level)` 存在，但 ZG330 等级语义**未获厂商确认**。

⇒ 系统级排队、准入、多模型分时、调用方公平性与恢复策略**全部归我方 Orchestration Layer**（见 `../../architecture/05-inference-orchestration.md` §3–§4）。

---

## 5.7 精度与模型能力（本板）

`[FACT-B]` 调查记录：

| 项 | 本板 |
|---|---|
| `qdtype` | int8 / bf16 / fp16 / tf32；**无 int4**；30TAI 不再支持 int16；当前**不支持 asymmetric** |
| ONNX 入口 opset | **17**（低于此解析报错） |
| 已知降级/缺失算子 | `Where` 在 ZG 侧**落 Host**；`Gather` 有定义但下降实现不完整；`RopeDynamic` **仅 Host**；**RMSNorm 未在所查规格中找到** |
| 模型黑名单 | 厂商 223 行模型表里实测 44 行不支持 |

**必须建立 `Operator × DType × Shape × CompilerVersion × RuntimeVersion` 能力矩阵。芯片支持精度 ≠ 每个算子/模型都支持。**

---

## 5.9 Tensor / DMA Buffer Manager（本板取值）

**跨机型契约见 `../../architecture/12-resource-lease-and-buffer-ownership.md` §12.4（所有权状态机与四条不变量）与 §12.5（地址域类型）。本节只列本板具体取值。**

本板地址域（`[FACT-A/B]`）：

| 类型 | 本板取值 |
|---|---|
| `CpuVirtualAddress` / `CpuPhysicalAddress` | 常规 |
| `DeviceDmaAddress` | 与 phys **相等**（厂商路径实测），但只作**启动时校验**，不作跨平台前提 |
| `NpuLogicAddress` | 由 NPU 内部 MMU 管理，**不等于 phys** |
| `PsDdrBuffer` | PS DDR 侧 buffer（`udmabuf`） |
| `PlDdrBuffer` | PL DDR 侧 buffer（NPU 主战场） |
| `UdmaBuffer` | `udmabuf0`，128 MiB 静态窗 |

本板 Cache 与同步（`[FACT-A]`）：

- **Cache 非一致**（实测直读 `/dev/udmabuf0` 不 sync 会拿到陈旧行）。
- `u-dma-buf` 的 `sync_for_cpu/device` 走 `dma_sync_single_for_*`。
- ICraft XRT 提供 `CacheSyncHostMemRegion`。
- **约 0.088 µs/op 是用户态 sysfs 写入开销口径**，不可当作所有设备 Cache 同步总延迟。

---

## 5.10 内存与性能约束（本板）

- PS DDR 1 GiB，Linux 可用约 **993 MiB**。
- CMA 配置 256 MiB，但静态 `udmabuf` 窗口 128 MiB，**实际剩余 CMA 约 128 MiB**。
- **单次完整 NPU 推理曾借用约 122 MB udmabuf**，接近窗口容量，需纳入并发/峰值计划。
- PL DDR 约 2 GiB，是 NPU 主要计算存储区域；PS↔PL 数据搬运需走已确认路径。
- PS DRAM memcpy **1359 MB/s** 是 CPU 侧读数，**不是 NPU 带宽**。
- PL AXI DMA `dmatest` **72.3/72.9 MB/s** 是 DMA 引擎 memcpy，**不是 NPU↔PLDDR 带宽**。
- **NPU 前向直接测量仍缺**；profiler 反解数字只能标为间接估计。

---

## 5.11 恢复策略（本板判定）

**分类与恢复等级契约见 `../../architecture/13-failure-classification-and-recovery.md`。本节只列本板判定。**

| 故障 | 本板恢复 | 证据 |
|---|---|---|
| 普通 Runtime 调用异常（如 `11015`） | **L1**：退出 Owner 进程 → CMA/句柄全回收 | `[FACT-A]` |
| `Device::reset(level)` | **L2 不可依赖**：level 0/1/≥2 反汇编行为已知，**非厂商语义保证**；挂死时往往执行不到 | `[FACT-A/B]` |
| NPU / 互连严重挂死 | **L3**：整板断电 | `[FACT-A]`，多次同签名 |

**本板附加禁令**：不得在普通测试中注入挂死类故障（只能断电，风险不可控）。

---

## 5.12 验收（本板）

- [ ] ICraft 3.39 头文件、动态库和示例**固定版本归档**。
- [ ] C ABI wrapper 能正确加载、调用、返回错误并释放资源。
- [ ] **单 Owner 仲裁有效**，多个客户端不会直接并发打开多个 Server。
- [ ] 运行中 timeout **不会误认为硬件已停止访问 Buffer**。
- [ ] CPU ORT 基线与 tract 候选在**目标模型**上比较。
- [ ] 模型支持矩阵与精度报告**带版本信息**。
- [ ] Buffer ownership / Cache Sync 有**单元 + 板端**测试。
- [ ] OOM、运行时错误、NPU hang **分级记录**；危险故障不在普通 CI 注入。

---

## 相关

- `../../architecture/05-inference-orchestration.md` —— 通用推理编排契约
- `../../architecture/12-resource-lease-and-buffer-ownership.md` —— Buffer 所有权
- `../../architecture/13-failure-classification-and-recovery.md` —— 故障分级
- `07-platform-baseline.md` §7.2–§7.4 —— 本板 ZG330 / DDR / DMA 事实
- `11-cpu-resource-allocation.md` —— 本板 CPU 核分配

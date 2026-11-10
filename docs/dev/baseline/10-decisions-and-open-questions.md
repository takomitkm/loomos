# 10 — 架构决策记录与开放事项

## 10.1 当前决策（可随着证据更新）

| ID | 决策 | 依据/边界 |
|---|---|---|
| ADR-001 | 近期以当前 Linux 为宿主内核，不启动 seL4/Theseus 迁移 | 当前平台调查和用户给出的三阶段 OS 规划 |
| ADR-002 | Rust 作为系统管理与安全 API 的主要开发语言 | 用户总体目标；厂商 C++ Runtime 通过 FFI 接入 |
| ADR-003 | ICraft 不视为系统级 Scheduler | 3.39.0 调查未发现 Scheduler API；forward 同步阻塞 |
| ADR-004 | NPU 单 Owner，系统级多请求编排由 Rust 负责 | 多 Server 并发不受支持，缺少 cancel/abort |
| ADR-005 | CPU/NPU/PL Buffer 所有权显式管理 | 当前非一致 Cache、共享物理窗无内核互斥、无运行时 IOMMU |
| ADR-006 | DMA 地址等于 phys 仅作为当前板/驱动观测值并启动时校验 | 不推广为跨平台假设；NPU logic_addr 另有内部 MMU |
| ADR-007 | 机器人只预留通用接口，不实现 ROS 2/机械臂子系统 | 当前无机械臂硬件/驱动/安全控制器 |
| ADR-008 | crate 版本在 Host/Cross/Hardware 验证后锁定 | 目标 AArch64/Ubuntu 20.04.4/厂商库 ABI 约束 |
| ADR-009 | 配置服务自研管理语义，复用成熟序列化/Schema/DB/通知 crate | 领域权限、Revision、Outbox、Desired/Applied 是系统语义 |
| ADR-010 | 不以当前旧代码结构为必须兼容目标 | 允许替换现有模块，架构由目标和证据决定 |

## 10.2 需要谨慎表述的事实

- SCHED_DEADLINE 准入测试通过，不等于证明所有未来混合任务集可调度。
- `PREEMPT_NONE` 下 FIFO 的特定负载尾延迟改善，不等于完整硬实时保证。
- `0.088 µs/op` 是 sysfs 写入测量口径，不等于整个设备 DMA/Cache 同步链总时间。
- 72.3/72.9 MB/s 是 PL AXI DMA `dmatest` memcpy，不是 NPU 前向或 PS↔PL↔NPU 带宽。
- 1108.7 MB/s / 约 660 MB/s 是间接 profiler 反解，不是干净直接带宽实测。
- PL 帧写路径 150.91 MB/s 是门级/行为仿真，不是板上实测。
- 设备 reset API 的反汇编行为不等于厂商承诺的恢复语义。
- 芯片支持的精度不等于 ICraft 对每个算子/模型均支持。

## 10.3 仍开放事项

| ID | 事项 | 是否阻塞当前主线 | 处理方式 |
|---|---|---|---|
| OPEN-01 | `Device::reset(level)` 厂商语义 | 不阻塞一般 OS Core；阻塞将其用作恢复策略 | 向厂商确认；确认前不使用 |
| OPEN-02 | NPU 前向干净直接带宽 | 不阻塞单 Owner/Orchestration 设计 | 仅对模型/算子支持的有效模型，在受控窗口测量 |
| OPEN-03 | PS↔PL↔NPU 完整数据路径带宽 | 不阻塞 Buffer API 初版 | 明确数据路径后分段测量，避免混用 DMA memcpy 指标 |
| OPEN-04 | CPU DW DMA 单通道数据 | 当前现役主要链不依赖它，不阻塞 | 有新用例再测 |
| OPEN-05 | PCB 丝印版本登记 | 不阻塞 | 下次现场登记 |
| OPEN-06 | 机器人具体硬件与安全机制 | 当前不属于开发范围 | 采购/确定型号后重新开领域设计 |
| OPEN-07 | ROS 2 与目标板 OS 兼容性 | 当前不阻塞 | 未来真要部署机器人时重评，不提前固定发行版 |
| OPEN-08 | tract 模型覆盖与性能 | 不阻塞 ORT 基线与 Inference API | 作为可选 CPU Backend 候选评测 |
| OPEN-09 | PREEMPT_RT 重编及厂商模块兼容 | 不阻塞当前非硬实时系统服务 | 有实时闭环需求后评估，需重出相关 `.ko` |
| OPEN-10 | SMMU DT 布线与真正设备隔离 | 当前用户态安全模型需明确限制；是否改造待论证 | 做威胁模型、IOMMU group 和 DMA 隔离验证 |

## 10.4 技术替换决策模板

每次更换 crate、Backend 或架构，记录：

```text
Decision ID:
Date:
Context / Workload:
Current option:
Alternatives:
Evidence / Measurements:
Chosen option:
Why:
Trade-offs:
Security / Failure impact:
Migration steps:
Rollback:
Validation required:
Status: proposed | accepted | rejected | superseded
```

## 10.5 不可违反的记录规则

1. 事实、推断、建议、裁定分开写。
2. 仿真、间接估算、CPU memcpy、DMA memcpy、NPU 前向带宽不得混为一谈。
3. 已闭合事项从待办中移除；不要反复要求用户重新验证已回答的问题。
4. 高风险板测必须有恢复窗口，不安排无人值守挂死注入。
5. 不因技术名词“支持”就推断目标板已可用；需区分源码配置、DT 布线、驱动 probe、运行时接口和实测行为。

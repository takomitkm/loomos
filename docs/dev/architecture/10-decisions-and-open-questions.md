# 10 — 架构决策记录与开放事项

> **层级**：本文是 `architecture/` 的**跨机型现行版**（编号镜像 `baseline/`）。本机型的具体读数与取舍见 `platforms/<机型>/` 下的同号文件；两者冲突时以本层为准并留记录。


## 10.1 当前决策（可随着证据更新）

| ID | 决策 | 依据/边界 |
|---|---|---|
| ADR-001 | 近期以当前 Linux 为宿主内核，不启动 seL4/Theseus 迁移 | 当前平台调查和用户给出的三阶段 OS 规划 |
| ADR-002 | Rust 作为系统管理与安全 API 的主要开发语言 | 用户总体目标；厂商 C++ Runtime 通过 FFI 接入 |
| ADR-003 | 厂商推理 Runtime 不视为系统级 Scheduler | 未发现 Scheduler API；`forward` 同步阻塞 |
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
- 芯片支持的精度不等于厂商 Runtime 对每个算子/模型均支持。

## 10.3 仍开放事项

**本机型的开放事项见 `../platforms/FMQL30TAI676H/` 下对应章节（如 `11-cpu-resource-allocation.md` §11.7）。** 本节只定「什么算开放事项」与记录格式：

| 要求 | 说明 |
|---|---|
| 可判定 | 每条 OPEN 必须写清「缺什么证据」与「谁来补」 |
| 分级 | 明确标注 阻塞主线 / 不阻塞 |
| 关闭留证 | 关闭时写清依据（实测、文档、还是裁定），不得只写"已解决" |
| 不得沉淀为口头约定 | 未进本表或机型页的"我们知道但没写"不算事项 |

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

## 10.6 平台化裁定与开放问题（2026-11-22 增补）

裁定（详情见对应 ADR）：
- **ADR-011 v1.1**：统一 Wasm 执行 + 主权边界修正——CapabilityStore/Lease/Supervisor/
  Lifecycle/配置权威/推理后端永留 Native Host；性能表述限定已测负载；
  「权威迁入 Wasm」方案否决并归档（archive/0001）。
- **ADR-012**：Nix 为 WAMR 执行职责之外全部依赖的单一声明权威；厂商工件四级分类；
  过渡期 APT 兜底不得宣称权威已替换；复现门=双 builder 独立重建+哈希比对。
- **ADR-013**：双槽 A/B + RAUC 优先评估 + 13 步事务 + 回退；板端分区本阶段不动。

开放问题（Open）：
1. flake.lock 首跑与 CI nix 接入（N0 收尾）——本机 Windows 无 Nix，无法本地验证。
2. loomis-host Linux 全量测试首跑结果（Nix buildRustPackage doCheck）——Windows 侧 163+ 全绿。
3. wamr-link 真链路验收：libiwasm.a（nix 产物）链接 + register_natives + nlu 组件
   执行 checksum 与 bench 基线（6159536122622086539）对齐。
4. WAMR native 签名字母表（i I f F v * $）与 register_natives 的 module_name 语义
   按文档确认，真机行为待 CI。
5. Windows/macOS WAMR 构建与 AOT 加载（MAP_32BIT 补丁工程化）——N4。
6. 板端 U-Boot bootenv/bootcount 语义调查——N5 前置（Unverified）。

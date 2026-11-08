# HA4FPAI / Rust Heterogeneous System — `riir` 规划入口

> 本目录是规划文档，不是现有实现说明。`riir` 分支/规划的目标是把系统设计、实施顺序、板端事实和验收记录集中起来。
>
> **现有代码是可复用资产，不是架构约束。** 可以保留、重构、替换或删除；决策依据是目标工作负载、真实硬件能力、故障边界和测量结果。

## 目标

以 Rust 作为系统管理与安全 API 的主要实现语言，在 FMQL30TAI（Cortex-A53 + ZG330 + FPGA PL）上构建异构智能家居系统。初期基于现有 Linux 5.4.52，不急于重写内核；先自研系统级资源、权限、组件、配置、推理协调与设备管理层。只有发现 Linux 无法满足的明确要求后，才提出最小内核改造。Rust 编译器扩展作为独立研究阶段，不作为近期前置条件。

机器人目前**没有采购、驱动器或安全控制器资料**。当前只预留与具体机器人技术无关的 Device / Operation / Lease / Telemetry / Fault / Execution Contract 接口；不开发 ROS 2、MoveIt、机械臂控制器或仿真系统，不让这些技术成为 OS 核心依赖。

## 文档导航

| 文件 | 内容 |
|---|---|
| [01-architecture.md](01-architecture.md) | 目标架构、边界、总体流程图 |
| [02-roadmap.md](02-roadmap.md) | 阶段路线、依赖关系、决策闸门 |
| [03-os-runtime.md](03-os-runtime.md) | Rust OS Runtime、Linux Backend、Capability、资源管理 |
| [04-configuration-service.md](04-configuration-service.md) | winregenv 借鉴、配置命名空间、Schema、事件和 crate 复用 |
| [05-inference-zg330.md](05-inference-zg330.md) | ICraft、NPU Orchestration、CPU/NPU、Tensor/DMA |
| [06-device-framework.md](06-device-framework.md) | 通用设备框架、异步操作、未来机器人预留接口 |
| [07-platform-baseline.md](07-platform-baseline.md) | 最新板上事实、已闭合问题、仍开放事项 |
| [08-validation-and-dependency-policy.md](08-validation-and-dependency-policy.md) | crate 准入、交叉编译、板上测试、CI |
| [09-execution-checklist.md](09-execution-checklist.md) | 可逐项执行的开发清单和验收标准 |
| [10-decisions-and-open-questions.md](10-decisions-and-open-questions.md) | 已定决策、待决事项、禁止误用的证据 |
| [evidence/](evidence/) | 用户提供的原始平台能力调查记录；其旧待办表由 07 文件中的最新增量覆盖 |

## 状态标记

- **[FACT-A]**：板上实测、生产运行读数或直接运行时证据。
- **[FACT-B]**：厂商文档、源码、头文件或二进制分析直接证据。
- **[INFERENCE-C]**：推断、间接读数或未经直接测量的判断。
- **[DECIDED]**：当前架构裁定。
- **[PROPOSED]**：建议方案，尚未实现/裁定。
- **[OPEN]**：需要补充证据或设计决策。
- **[FUTURE]**：未来领域预留，不属于当前开发范围。

证据等级描述的是证据来源，不表示实现成熟度。某个 A 级事实也可能只是发现了限制。

## 当前最重要的边界

1. Linux 5.4.52 当前没有可用 cgroup 控制器、Namespace、seccomp 和 LSM；不能照搬通用 Linux 主机的隔离假设。
2. ICraft 3.39.0 没有已证实的系统级 Scheduler；`forward` 同步阻塞，无已确认的任务取消接口，多 Server 并发不受支持。
3. NPU/PL 与 PS DDR 非硬件一致性；交接必须显式 Cache Sync 和所有权转移。
4. 当前运行时没有跨设备 IOMMU 隔离；PL Master 可访问整段 PS DDR，Bitstream/PL 逻辑属于可信硬件边界。
5. 机器人只预留通用设备接口；未来 ROS 2 等由独立 Domain Adapter 接入。
6. crate 版本先候选、再 x86 测试、AArch64 交叉构建、目标板运行验证，最后锁定 `Cargo.lock`。

## 建议阅读顺序

`01 → 02 → 07 → 03 → 05 → 04 → 06 → 08 → 09 → 10`。

先理解目标和硬件事实，再确定模块与具体实施。不要先创建所有目录或先写抽象接口；每个抽象都应有实际调用方、测试和验收条件。

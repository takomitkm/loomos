# 07 — FMQL30TAI / ZG330 当前平台能力基线

> 事实来源：用户提供的《ZG330平台能力边界》及 2026-11-08 后续增量板测。该文档原始版本的 §七 待办表早于后续测试，**必须以本页更新状态为准**。证据等级：A=板上实测/生产读数；B=厂商文档、源码或二进制直证；C=推断/间接读数。

## 7.1 板卡与 Linux

| 项目 | 当前结论 | 证据 |
|---|---|---|
| 板卡 | FMQL30TAI676H 悟净·完整版；DT 自报 `FMQLMP FMQL100TAI` 是参考树兼容标识，不代表贴片型号 | A |
| Boot | FSBL + bl31 + U-Boot 2022.10 | A |
| OS | Ubuntu 20.04.4 LTS focal, aarch64 | A |
| Kernel | 厂商 Linux 5.4.52，ARCH_FMQLMP | A |
| CPU | 4× Cortex-A53 r0p4，标称最高 1 GHz；运行时无 cpufreq 节点/OPP | A |
| Timer | HZ=250、HIGH_RES_TIMERS、NO_HZ_IDLE（非 FULL）、SCHED_HRTICK | A |
| Preemption | `PREEMPT_NONE`，当前不是 PREEMPT_RT | A |
| IRQ | `IRQ_FORCED_THREADING=y`，可通过 `threadirqs` 参数启用线程化（排除 IRQF_NO_THREAD） | A |
| cgroups | `CGROUPS=y`，但 SCHED/PIDS/MEMCG/CPUSETS/DEVICE/CPUACCT controllers 均未编译；`/proc/cgroups` 无控制器 | A |
| Namespace/seccomp/LSM | NAMESPACES、SECCOMP、SECURITY 未启用；全 root 运行现状需整改 | A |

### 实时调度更新

- [FACT-A] `sched_getscheduler` 实测 DL/FIFO/RR policy 编号为 6/1/2。
- [FACT-A] `sched_rt_period_us=1000000`、`sched_rt_runtime_us=950000`。
- [FACT-A] DL 参数负例被 `EINVAL` 拒绝；4 个 90% DL 任务并发准入测试通过。
- [FACT-A] `/proc/self/sched` 不存在原因是 `SCHED_DEBUG` 未编译，不代表调度器不可用。
- [FACT-A] CFS max 10.803 ms vs FIFO 绑核 max 1.124 ms，是特定测试负载读数，不是硬实时证明。
- [FACT-A] 现役生产策略包含 `taskset`、`nice`、SIGSTOP/SIGCONT 占空比；既有 ASR RTF 0.80、视频 30fps 等场景数据。

## 7.2 ZG330 / ICraft

| 项目 | 当前结论 | 证据 |
|---|---|---|
| 版本 | CLI `3.39.0.0-431be90`，板端 Runtime v3.39.0 compat=true | A |
| API | Session/Device/Tensor/Backend/MemRegion/MemManager 等 | B |
| Scheduler | 当前调查未发现 ICraft 系统级 Scheduler；CreateByOrder 是后端尝试顺序，不是任务调度 | A/B |
| Forward | 同步阻塞 | A/B |
| Cancel | 未发现 cancel/abort API；11015 为超时相关错误之一 | A/B |
| Multi-Server | 厂商文档不建议同时打开多个 Server | B |
| Priority 参数 | `FForwardBackend` 含 `uint8_t priority`，但语义未解释，不得依赖 | B/C |
| Buffer API | MemRegion::malloc、Tensor::mallocOn、外部 MemPtr/ExternalMem 等路径 | B |
| Cache API | `CacheSyncHostMemRegion` 存在 | B |
| Reset API | `Device::reset(level)`/check/showStatus 存在 | B |
| Reset 反汇编 | level 0 全量脉冲并调用 initAfterRegions；level 1 轻量脉冲；>=2 静默 no-op | A（二进制分析） |

**Reset 限制：** 反汇编行为不等于厂商定义语义。level 含义未获得厂商确认前，不把该 API 当作正式故障恢复路径。

## 7.3 NPU 故障与恢复（最新裁定）

- 11015 相关超时源位于 DR_CFG0/23 写入超时路径；对“超时后能否继续运行”的问题，现有实际故障经验已经给出结论：NPU/互连挂死后不能依赖进程级恢复，需整板断电。
- 普通 Runtime 调用异常/进程退出可回收进程拥有的 CMA/句柄；这与设备互连挂死是不同故障等级。
- Qwen2-0.5B decoder 当前编译件存在 HardOpNode 11603 未绑定后端（11013），不能用该模型测 NPU 前向带宽。
- 不再为已经确认的 NPU hang 安排危险故障注入。

## 7.4 DDR / CMA / DMA / IOMMU / Cache

| 项目 | 当前结论 | 证据 |
|---|---|---|
| PS DDR | `0x00000000–0x3fffffff`，1 GiB 物理窗；Linux 可用约 993 MB；另有 1 GiB swapfile | A |
| CMA | 配置 256 MiB；128 MiB `udmabuf0` 静态划出（@ `0x30100000`），有效剩余约 128 MiB | A |
| NPU 单次内存 | 完整推理曾借走约 122 MB udmabuf，接近窗口容量 | A |
| PL DDR | 2 GiB 已由厂商手册/颗粒资料确认；MIG 为 `mig_7series_0`，64-bit、UI 200 MHz；剩余突发等 RTL 细节非当前阻塞 | A/B |
| NPU 内存路径 | PLDDR 直接可达；PS DDR 经厂商 CDMA/udma 路径；自研 PL Master 经 HP 可访问 PS DDR | B |
| DMA 控制器 | 厂商 CDMA；PS DW DMA `e004b000` 8 通道；PL DesignWare AXI DMA `f0010000` 8 通道 | A |
| DMA 地址 | 当前板厂商路径观测 DMA addr==phys；NPU `logic_addr` 经内部 MMU，和 phys 不同 | A/B |
| IOMMU/SMMU | SMMU 驱动在内核配置中，但 DT 未布线、无运行时 IOMMU groups；当前无跨设备 DMA 隔离 | A |
| Cache | 非硬件 I/O 一致；udmabuf `sync_for_cpu/device` 调用 `dma_sync_single_for_*`；XRT 有 `CacheSyncHostMemRegion` | A/B |
| Buffer 同步 | 所有权交接必须显式同步；不能假定 CPU/NPU/PL cache coherent | A |
| 当前所有权 | 内核不对多个 mmap 共享物理窗做互斥；生产链使用单 Owner/frame_lease 等机制 | A |

### 最新带宽与同步结果

- PS DRAM memcpy：1359 MB/s，属于 CPU 侧 memcpy 测量，不是 NPU 带宽。
- PL AXI DMA `dma1chan0`：`dmatest` 64 KiB×20 为 72.3 MB/s；1 MiB×10 为 72.9 MB/s。它测的是内核 DMA 引擎 memcpy，不是 NPU↔PLDDR 跨边界带宽。
- PS DW DMA `dma0chan0`：没有拿到干净单通道结果；目前不是现役主要数据通路的阻塞项。
- NPU 前向带宽：没有干净直接测量；profiler 反解的 1108.7 MB/s / 约 660 MB/s 仅是间接估计，禁止标成板上直接实测。
- CacheSync：最新测量约 0.088 µs/op，口径是用户态 sysfs 写入开销；实际同步在内核 DMA sync 路径执行，不能把该值解释为所有 Cache 操作的总成本。
- 已废弃数据：Python 逐字节 sum 得到的 21.5/21.9 MB/s；16 通道并发抢总线 3.9–6.8 MB/s；不能引用为链路能力。

## 7.5 PL / 视频 / 位流

- GP0/GP1 是 PL AXI 从设备窗口，不是 DDR 内存。
- 位流在启动链加载，FS 中无 `.bit` 文件；目前没有确认有效的运行时重载路径。
- 位流不匹配访问可能导致 AXI 互连挂死且无 printk，已有多次同签名经验；不要在普通测试中探测未知寄存器。
- 150.91 MB/s 是 `S_AXI_HP2` 64-bit×100 MHz 写帧器门级/行为仿真结果，不是板上实测。
- 1080p 裁剪到 1024/960×512 的既有原因是 PL 缩放整数步长及模型高度 32 对齐约束，不应错误归因于带宽不足。

## 7.6 机器人与 ROS

- [FACT-A] 当前板上没有 ROS 2。
- [FACT-A] 当前项目未登记机械臂、驱动器、安全控制器。
- [DECIDED] 当前只预留 Device Core 通用接口；机器人是未来 Domain，不进入当前部署与开发主线。

## 7.7 已闭合与剩余事项

| 事项 | 状态 | 备注 |
|---|---|---|
| SCHED_DEADLINE 冒烟/sysctl | 已闭合 | 上文记录实际数值与限制 |
| CacheSync 触发链/开销 | 已闭合 | 0.088 µs/op 为用户态写 sysfs 口径 |
| NPU hang 是否可无 reset 续跑 | 已闭合 | 严重挂死需断电，不重复危险注入 |
| CPUIdle 深睡延迟 | 已闭合 | 缺 `CONFIG_ARM_PSCI_CPUIDLE`，无 C-state 注册；深睡不存在 |
| Watchdog DT | 已闭合 | 驱动在但 DT 无节点，当前无 `/dev/watchdog*` |
| `Device::reset(level)` | 部分闭合 | 反汇编行为已知，厂商语义仍未知，不作为恢复方案 |
| PL DDR 容量 | 基本闭合 | 2 GiB 已确认，剩少量 RTL 细节 |
| PS↔PL↔NPU 带宽 | 部分开放 | DMA 测量与间接读数已有；NPU 前向直接测量仍缺 |
| tract 评估 | 未开始 | 只有选为 CPU 后端候选时才继续 |
| ROS 2 目标板适配 | 未开始/非当前范围 | 机器人无当前硬件需求 |
| PCB 丝印版本 | 待现场登记 | 低优先级 |

## 7.8 来源与更新纪律

- 原始调查版本：`evidence/ZG330平台能力边界-原始调查记录.md`。
- 本页包含后续增量，优先级高于原始记录旧 §七 待办表。
- 新板测必须记录日期、内核/Runtime 版本、命令、原始输出、负载状态和是否影响现役视频/NPU 链路。
- 测量值必须标明：直接实测、源码/文档证据、仿真、间接估计、无效读数。

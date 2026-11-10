# 03 — OS Runtime：本板差异

> **层级**：通用设计见 `../../architecture/03-os-runtime.md`（Runtime 模块、Capability、Component/Lifecycle、恢复原则）。
> **本文只列本板与通用版不一致的地方**，不重复定义。

---

## 3.3 Resource Model：本板取值

对应 `../../architecture/12-resource-lease-and-buffer-ownership.md` §12.2–§12.3。

| Kind | 本板实际承载 | 约束 |
|---|---|---|
| `BufferLease` | **`udmabuf`**（128 MiB 静态窗，@ `0x3010_0000`） | 生命周期由 `frame_lease` 单所有者租约保证；内核**不对多进程 mmap 同一物理窗做互斥** |
| `DeviceLease` | NPU（`npuserv` 单 Owner）、相机 | 同上，全靠用户态租约 |
| `CpuBudget` | **只有全局 `sched_rt_runtime_us=950000` 一把尺**（cgroup CPU 控制器未编译） | 等级须报 `best_effort`/`unsupported`，见 `11-cpu-resource-allocation.md` §11.3 |
| `MemoryLimit` | `RLIMIT_AS` | **不等同物理内存全局配额** |
| `NpuSessionLease` | ICraft Session/Backend（Owner 内部） | 单 Owner，不允许多 Server 并发 |

**`CPUQuota`/`MemoryMax` 是本板假闸门**：systemd 静默接受，但对应 cgroup 控制器未编译 ⇒ 必须报 `unsupported`（举证见 `../../architecture/11-execution-and-resource-model.md` §11.3.2）。

---

## 3.6 Supervisor / Recovery：本板恢复等级

对应 `../../architecture/13-failure-classification-and-recovery.md` §13.3。

| 等级 | 本板 | 判据 | 证据 |
|---|---|---|---|
| **L1 进程重启** | ✅ 支持 | Runtime 异常（如 `11015`）→ 退出 Owner 进程 → CMA/句柄可全回收 | `[FACT-A]` |
| **L2 设备 reset** | ⚠️ **不可依赖** | `Device::reset(level)` 反汇编行为已知，但**厂商语义未确认**；挂死时往往执行不到 | `[FACT-A/B]` |
| **L3 整板断电** | ✅ 唯一可靠 | NPU/互连挂死需断电，多次同签名实测 | `[FACT-A]` |

**本板附加禁令**：不得在普通测试中注入挂死类故障（只能断电，风险不可控）。

---

## 3.7 Linux Backend：本板映射

| Runtime 能力 | 本板做法 | 限制 |
|---|---|---|
| Process Isolation | 独立进程 + MMU | 当前进程服务多以 root 运行，需收缩身份和设备访问 |
| CPU Scheduling | Linux Scheduler、`taskset`、`nice`、`chrt` | `PREEMPT_NONE`；不能承诺硬实时。核分配落地见 `11-cpu-resource-allocation.md` |
| Resource Accounting | 用户态监控、`RLIMIT` | cgroup controllers 为空 |
| Memory Limit | `RLIMIT_AS` | 需上板验证具体进程/映射行为 |
| Device Access | 单 Owner 服务、设备节点权限、Lease | 当前 LSM/Namespace/seccomp 不可用 |
| IPC | HTTP/TCP 控制面 + udmabuf/共享内存数据面 | 需要协议版本、所有权、同步和错误模型 |
| DMA | Linux/vendor DMA API 与现有驱动 | 无跨设备 IOMMU 隔离 |
| Lifecycle | systemd | 是监督工具，不是完整安全管理器 |

---

## 相关

- `../../architecture/03-os-runtime.md` —— 通用 OS Runtime
- `../../architecture/11-execution-and-resource-model.md` —— 核亲和与实施等级
- `../../architecture/12-resource-lease-and-buffer-ownership.md` —— 租约与 Buffer 所有权
- `../../architecture/13-failure-classification-and-recovery.md` —— 故障分级与恢复
- `07-platform-baseline.md` —— 本板平台事实
- `11-cpu-resource-allocation.md` —— 本板核分配

# 03 — Rust OS Runtime 与 Linux Backend

> **层级**：本文是 `architecture/` 的**跨机型现行版**（编号镜像 `baseline/`）。本机型的具体读数与取舍见 `platforms/<机型>/` 下的同号文件；两者冲突时以本层为准并留记录。


## 3.1 当前 Linux 平台事实约束

目标机型的内核/隔离能力差异极大，**不能套用通用 Linux 主机的隔离假设**。每个机型必须先给出自己的能力基线（内核版本、抢占模型、cgroup 控制器、Namespace/seccomp/LSM、调度策略），再据此决定哪些约束可 `enforced`。

> 本板基线见 `../platforms/FMQL30TAI676H/07-platform-baseline.md` §7.1。

当前可用的执行机制包括进程/MMU 基础地址空间隔离、FIFO/RR、已实测的 DEADLINE 准入、`taskset`、`nice` 和用户态调度策略。具体见 `07-platform-baseline.md`。

## 3.2 Runtime 模块

```text
os-runtime/
├── ids/                 # ComponentId, ResourceId, ExecutionId 等
├── resource/            # 资源描述、准入、Lease、Accounting
├── capability/          # 权限对象、委托、撤销、审计
├── component/           # Manifest、依赖、注册
├── lifecycle/           # 状态机和转换规则
├── supervisor/          # 健康检查、启动、退出、重启
├── recovery/            # 资源回收、状态重建、恢复策略
├── scheduler/           # Work Dispatcher / 策略，不替代 CPU Scheduler
├── ipc/                 # 控制消息和数据通道抽象
├── configuration/       # 独立配置服务
└── platform/
    └── linux/           # 当前 Linux 系统调用/文件/驱动适配
```

## 3.3 Resource Model

资源不应被统一成一个 `u64 quota`。至少区分：

| Resource Kind | 语义 | 例子 |
|---|---|---|
| CpuBudget | 一段时间内可消耗 CPU 时间 | runtime/period/deadline |
| MemoryLimit | 容量上限 | RSS/address-space 限制 |
| BufferLease | 对某块内存的生命周期与访问权 | 共享内存块、共享帧 |
| DeviceLease | 设备访问/控制权 | NPU、相机、未来机器人 |
| NpuSessionLease | 厂商 Runtime Session/Backend 资源占用 | NPU Owner 内部 |
| StorageQuota | 持久存储资源 | 日志、模型缓存 |
| NetworkBudget | 网络连接/带宽策略 | 控制面通信 |

Resource Manager 决定申请、准入、租约和回收；Backend 负责实施可用的限制并报告执行等级：`enforced`、`best_effort`、`unsupported`。不能将未能强制实施的约束报告为 enforced。

## 3.4 Capability Model

推荐的逻辑对象：

```rust
struct CapabilityHandle<T> {
    object_id: ObjectId,
    rights: Rights,
    generation: u64,
    _type: PhantomData<T>,
}
```

这是概念草案。类型参数可防止 API 误用，但真正权限仍需由服务端/内核校验。动态撤销需 generation/revocation state，不能仅依赖 Rust Lifetime。

权限至少区分：

- `config.read(namespace)` / `config.write(namespace)`。
- `device.observe(device_id)` / `device.command(device_id)`。
- `resource.request(kind)` / `resource.admin`。
- `npu.submit` / `npu.admin`。

不要把全局 Resource Admin Capability 发给普通业务服务。

## 3.5 Component / Lifecycle

建议组件状态：`Registered → Configured → Ready → Active → Suspended/Failed → Recovering → Terminated`。状态转换应有明确的前置条件和事件。设备状态与管理设备的服务进程状态分开建模。

Component Manifest 描述：身份、版本、依赖、允许的 Capability、资源请求、启动方式、健康检查和恢复策略。第一版可以用 TOML/YAML 文件加载，运行时解析为类型化数据；格式本身不构成安全边界。

## 3.6 Supervisor / Recovery

- 正常进程崩溃：记录退出原因，回收 Lease，按策略重启。
- OOM：记录资源压力，拒绝无约束重启风暴。
- Backend 超时：区分请求超时、进程卡死、设备挂死。
- 设备挂死：机型必须声明能否进程级恢复；已知严重挂死可能需整板断电（见 `../platforms/<机型>/`）。
- 设备重新上线：重新探测状态、版本、资源所有权和安全状态，不自动重放危险命令。

恢复策略必须按故障类别定义，禁止统一 `catch Exception → restart()`。

## 3.7 Linux Backend 当前映射

| Runtime 能力 | 当前做法 | 限制 |
|---|---|---|
| Process Isolation | 独立进程 + MMU | 当前进程服务多以 root 运行，需收缩身份和设备访问 |
| CPU Scheduling | Linux Scheduler、taskset、nice、chrt | PREEMPT_NONE；不能直接承诺硬实时 |
| Resource Accounting | 用户态监控、RLIMIT | cgroup controllers 当前为空 |
| Memory Limit | RLIMIT_AS 等可用限制 | 需上板验证具体进程/映射行为；不等同物理内存全局配额 |
| Device Access | 单 Owner 服务、设备节点权限、Lease | 当前 LSM/Namespace/seccomp 不可用 |
| IPC | 现有 HTTP/TCP 控制面 + 共享内存数据面 | 需要协议版本、所有权、同步和错误模型 |
| DMA | Linux/vendor DMA API 与现有驱动 | 当前运行时无跨设备 IOMMU 隔离 |
| Lifecycle | systemd | 是监督工具，不是完整安全管理器 |

当前生产的 `taskset + nice + SIGSTOP/SIGCONT` 占空比策略是已存在的工程手段，不等于通用调度 API。应记录用途与负载，避免随意推广。

## 3.8 现在不做的内核工作

不立即实现 TCB、Context Switch、MMU、IPC Kernel 或 Native Scheduler。先验证 Linux 后端能否满足已知需求。内核改造优先评估：打开已有安全机制、完善 SMMU/DT、实时内核适配和厂商模块重编成本。

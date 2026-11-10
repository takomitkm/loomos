# 11 — 执行与资源模型：核亲和、预算与实施等级

> **层级**：本文是 `architecture/` 的**跨机型现行版**（编号镜像 `baseline/`）。本机型的具体读数与取舍见 `platforms/<机型>/` 下的同号文件；两者冲突时以本层为准并留记录。
>
> **文档定位**：`architecture/` 是**跨平台架构约束与接口规范**——这里的规定**对所有机型成立**，不随硬件变化。
> `baseline/` 是通用设计原稿（**永久冻结**，不针对具体硬件修改）；`platforms/<机型>/` 是各机型的具体落地与证据。
> **本文只定「接口长什么样、什么必须成立、怎么判合规」；本板怎么做到、读数多少，写在 `platforms/` 里。**

---

## 11.1. 三层分工

| 层 | 管什么 | 能不能改 |
|---|---|---|
| `baseline/` | 通用设计原稿：对象、流程、原则 | **冻结，不再改** |
| `architecture/`（本文所在） | 跨机型**约束与接口**：对象属性、不变量、等级契约、适配模板 | 可演进；改动须记录 |
| `platforms/<机型>/` | 该机型**怎么满足**上述约束 + 实测证据 + 本机分配表 | 随该机型演进 |

**一条判据**：把一句话搬到另一块板上，它**仍然成立**⇒ 属于 `architecture/`；**不成立**⇒ 属于 `platforms/`。

---

## 11.2. 对象模型

### 11.2.1 `ExecutionContext`

执行体的运行约束。**核亲和属于这里，不属于 `Resource Kind`**（依据见 §3.1）。

```text
ExecutionContext {
    kind:       Thread | Process | DeviceCtx,
    affinity:   CoreSet,      // 允许运行的核集合（"允许集"，不是"必须在其上"）
    inherit:    bool,         // 派生的执行对象是否继承本 affinity
}
```

语义：

- `affinity` 是**允许集**：调度器仍可在集合内迁移，不保证钉死在某一核。
- **空集非法**；元素必须落在该机型声明的 `cores_total` 范围内。
- `affinity` 未设置时继承默认值（见 §4），不表示"随便哪个核"。

### 11.2.2 `ExecutionContract`

准入条件。与 `ExecutionContext` 的关系是**请求 vs 能力**。

```text
ExecutionContract {
    affinity_required: Option<CoreSet>,   // 准入条件：满足才放行
    budget:           Option<CpuBudget>,  // 时间预算，语义沿用 baseline/03 §3.3
}
```

- `affinity_required` 必须是 `ExecutionContext.affinity` 的子集；**超集即拒绝**（§3.3）。
- `budget` 的定义不在本文重复，见 `baseline/03-os-runtime.md` §3.3。

### 11.2.3 `CoreSet`

- 核编号的有序整数集合（或等价位图）。
- 机型必须声明 `cores_total`；**不允许空集**，**不允许越界编号**。
- 序列化格式须稳定、可比较（用于准入时的子集判定）。

---

## 11.3. 三条不变量

### 11.3.1 核亲和是执行对象属性，不是 `Resource Kind`

**依据（微内核参考设计）**：

| 设计 | 绑核机制 | 所在层 |
|---|---|---|
| **seL4** | `seL4_TCB_SetAffinity(tcb, core)` | **TCB（线程）上的能力调用** |
| **QNX Neutrino** | `ThreadCtl(_NTO_TCTL_RUNMASK, runmask)`（核位图） | **线程属性** |
| **seL4 MCS** | `SchedControl` **每个 CPU 一个**——调用哪个核的 `SchedControl`，就把该调度上下文绑到哪个核 | **预算天然带核归属** |

seL4 文档原文（[MCS 教程](https://docs.sel4.systems/Tutorials/mcs.html)）：

> The invoked `seL4_SchedControl` determines **which processing core** that specific scheduling context provides access to.

**推论（三条，均为约束）**：

1. **不新增 `CpuCore` 一类的 `Resource Kind`。** `Resource Lease` 管的是**可让渡的所有权**（设备、Buffer、Session）；核不是可让渡物，它是执行体的运行约束。
2. **预算必须与核归属成对出现。** 只给 `budget` 而不给核归属，等于把 seL4 里 `SchedControl` 绑定的两件事拆散——预算会退化成全局一把尺。
3. **不要把 NPU Job、设备 Lease、CPU Thread 混成一种调度对象**（沿用 `baseline/01-architecture.md` §1.6 的自警）。

### 11.3.2 实施等级必须如实上报

`baseline/03-os-runtime.md` §3.3 要求 Backend 报三级，本文给出**举证要求**：

| 等级 | 含义 | 举证要求（缺一不可） |
|---|---|---|
| **`enforced`** | 平台能强制，且已验证 | 必须有「设置后读回」或「设置前后对照」的实测证据 |
| **`best_effort`** | 只能尽力，可能失效 | 必须写明**失效条件** |
| **`unsupported`** | 平台无此能力 | 必须有「设了但不生效」的**对照证据**，不能只凭"配置项不存在"下结论 |

**红线两条**：

1. **不得把未强制实施的约束报告为 `enforced`**（沿用 baseline）。
2. **「接口存在」≠「能力生效」。** 假闸门是典型：systemd 会**静默接受** `MemoryMax`/`CPUQuota`，即使对应 cgroup 控制器根本没编译——这种必须报 `unsupported`，不得报 `enforced`。

### 11.3.3 准入不满足即拒绝，不静默降级

- `affinity_required` 不满足 ⇒ **拒绝准入，返回明确错误**。
- **禁止**"悄悄放到别的核上跑"。
- 理由：静默降级会让性能问题无法归因——旧式「占空比」策略难排查，根因就是它没有可观测的拒绝点。

---

## 11.4. 继承语义

| 场景 | 默认 | 可配置 | 先例 |
|---|---|---|---|
| 父执行对象创建子对象 | 子继承父的 `affinity` | `inherit=false` 关闭 | QNX `inherit mask` |
| 服务端处理客户端请求 | 服务执行对象**可**继承调用方的 `affinity` | 按接口配置 | QNX `_NTO_CHF_INHERIT_RUNMASK` |
| 显式设置 | 覆盖继承值 | — | — |

**为什么需要继承**：服务进程通常不知道自己被谁调用、该跑哪个核；让调用方决定比让服务硬编码更合理，也避免每个线程各自设一遍。

---

## 11.5. 各机型适配表（必填模板）

每个机型必须在自己 `platforms/<机型>/` 下给出下表。**缺任一行即视为该机型未完成适配。**

| 项 | 要求 |
|---|---|
| `cores_total` | 核数，及是否有调频/OPP |
| **机制清单** | 逐条列出并标 available 与否 + 证据：`sched_setaffinity`／`sched_setscheduler`／cgroup CPU／cgroup CPUSETS／`RT_GROUP_SCHED`／`SCHED_DEADLINE`／`isolcpus`／cpufreq |
| **等级判定** | `ExecutionContext.affinity`、`ExecutionContract.affinity_required`、`budget`、`priority` 各自报 `enforced`/`best_effort`/`unsupported` |
| **核分配表** | 哪个核归谁、冲突怎么处理、是否持久化 |
| **验收读数** | 至少一条「核归属带来多少收益」的**本机实测**（不接受引用他机数值） |

---

## 11.6. 与 `baseline/` 的关系

- 本文**不重新定义** `CpuBudget`、`Resource Lease`、`Capability`、`Execution Contract` 的通用语义——那些在 `baseline/`。
- 本文只补 baseline 未覆盖的两件事：**执行对象的核亲和属性**、**实施等级的举证规则**。
- 若本文与 `baseline/` 冲突，**以本文为准**（architecture 是活文档，baseline 是冻结原稿）；但冲突必须在本文留一条记录，不得静默覆盖。

---

## 11.7. 相关

- `baseline/01-architecture.md` §1.6 —— `Execution Context` / `Execution Contract` / `Resource Lease` 的对象定义（本文的属性挂载点）
- `baseline/03-os-runtime.md` §3.3 / §3.7 —— `CpuBudget` 定义与 Linux Backend 映射；本文补其举证规则
- `platforms/FMQL30TAI676H/11-cpu-resource-allocation.md` —— **本板落地**：机制盘点、等级判定、核分配表、实测读数

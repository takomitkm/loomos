# 11 — CPU 核分配与调度：FMQL30TAI676H 本板落地

> **文档定位**：`baseline/` 是通用设计原稿（**永久冻结**）；`architecture/` 是跨机型约束与接口；**本文是本机型的具体落地 + 证据**。
> **对象模型、不变量、等级契约、继承语义、适配表模板一律见 `../../architecture/11-execution-and-resource-model.md`，本文不重复定义**（仓库规矩：一个事实只写一处）。
> 本文只回答：**这块板上有什么、能强制什么、分给谁、实测多少、怎么验收。**
>
> 证据等级：`[FACT-A]` 板上实测 ／ `[FACT-B]` 厂商文档·源码·二进制直证 ／ `[INFERENCE-C]` 推断未测 ／ `[PROPOSED]` 建议未裁定 ／ `[OPEN]` 待补证据。

---

## 11.1 本板事实

| 项 | 本板 | 证据 |
|---|---|---|
| `cores_total` | **4**（Cortex-A53 r0p4，标称 ≤1 GHz） | `[FACT-A]` |
| 调频 | **无 cpufreq 节点/OPP**（`CPU_FREQ` 框架在、DT 未给 OPP）⇒ 不可调也不可读 | `[FACT-A]` |
| 抢占模型 | **`PREEMPT_NONE`**，非 RT | `[FACT-A]` |
| 定时器 | `HZ=250`、`NO_HZ_IDLE`、`SCHED_HRTICK` | `[FACT-A]` |
| cgroup CPU 控制器 | **未编译**（`/proc/cgroups` 零行） | `[FACT-A]` |
| cgroup CPUSETS | **未编译** | `[FACT-A]` |
| `RT_GROUP_SCHED` | 未编译 ⇒ 无 per-group RT 带宽，只有全局 `sched_rt_runtime_us=950000` | `[FACT-A]` |
| `isolcpus=` | **未设**（`/proc/cmdline` 无） | `[FACT-A]` |

### 11.1.1 争核的常驻服务

| 服务 | 负载特征 | 现状 |
|---|---|---|
| 视频链（VPU 编码／预览） | 持续 30 fps | **已 `taskset` 钉 cpu0** `[FACT-A]` |
| 相机人脸（`wcis-face-photo.service`） | 实测约 **71% 的一个核** | `CPUAffinity` 为空（未设） `[FACT-A]` |
| ASR | 突发整句推理 | `taskset` + `nice` + SIGSTOP/SIGCONT 占空比 `[FACT-A]` |
| TTS（如启用） | 突发数秒合成 | 待定 |

### 11.1.2 旧版的解法

`[FACT-A]`，记录在 `ZG330平台能力边界-原始调查记录.md`：

> `taskset`＋`nice`＋SIGSTOP/SIGCONT 占空比——全部生产实证（ASR 抢核 RTF 0.80、**视频链钉 cpu0 满帧 30fps**、闲时压缩 0.15s/0.85s）

**核心是静态核归属：谁归哪个核，硬分。**

### 11.1.3 本板在通用模型下的具体缺口

`architecture/11-execution-and-resource-model.md` §11.2 定义了 `ExecutionContext.affinity`。**本板目前没有任何机制去表达或强制它**——现有 `baseline` 的 `CpuBudget` 只有时间维度，而本板的时间维度强制手段（cgroup）**全部不可用**（见 §11.2）。

---

## 11.2 本板机制盘点：什么真能强制

| 机制 | 本板状态 | 能否强制 | 对应微内核原语 |
|---|---|---|---|
| **`sched_setaffinity`** | ✅ 可用（`taskset` / `pthread_setaffinity_np`） | **✅ enforced** | seL4 `TCB_SetAffinity` / QNX runmask |
| **`sched_setscheduler`（FIFO/RR）** | ✅ 可用（`RLIMIT_RTPRIO` + 全局 `sched_rt_runtime_us`） | ✅ enforced（**全局预算**，非 per-group） | seL4 优先级 / QNX 优先级 |
| **`SCHED_DEADLINE`** | ✅ 主线无条件编译 + HRTICK | ⚠️ 自带全局带宽制 | seL4 MCS 预算（近似，但全局） |
| cgroup `CPUQuota` | ❌ CPU 控制器未编译 | ❌ **假闸门**：systemd 静默接受、完全无效 | — |
| cgroup `CPUSETS` | ❌ 未编译 | ❌ 无法划核 | seL4 per-CPU `SchedControl`（**无对应物**） |
| `RT_GROUP_SCHED` | ❌ 未编译 | ❌ 无 per-group RT 带宽 | — |
| `isolcpus=` | 未设 | ⭕ 可加但**需改内核 cmdline + 重启** | — |
| 调频（governor） | ❌ 无 cpufreq | ❌ 路封死 | — |

**⇒ 本板真正 `enforced` 的执行控制只有两条：`sched_setaffinity`（核）与 `sched_setscheduler`（FIFO/RR 优先级）。**

---

## 11.3 本板适配表（按 architecture §5 模板填）

### 11.3.1 等级判定

| 抽象 | 本板实现 | **必须上报的等级** | 备注 |
|---|---|---|---|
| `ExecutionContext.affinity` | `sched_setaffinity` | **`enforced`** | 唯一真能强制的核手段；设置后可用 `taskset -pc <pid>` 读回 |
| `ExecutionContract.affinity_required` | 同上（准入时校验） | **`enforced`** | 不满足即拒绝 |
| `ExecutionContract.budget` | cgroup CPU 控制器 | **`unsupported`** | 未编译；**不得报 enforced**（假闸门） |
| `ExecutionContract.budget` | `SCHED_DEADLINE` | `best_effort` | 全局带宽制，非 per-component |
| `ExecutionContext.priority` | `sched_setscheduler(FIFO)` | `enforced` | 受全局 `sched_rt_runtime_us=950000` 约束 |

**假闸门的举证**（对应 architecture §3.2 红线 2）：systemd `CPUQuota`/`MemoryMax` 在本板被**静默接受**（无报错、`systemctl show` 有值），但对应 cgroup 控制器未编译 ⇒ 限额完全不生效。这是 `unsupported` 的对照证据，不是 `enforced`。

### 11.3.2 继承参数的落地取值

| 场景 | 本板建议取值 | 依据 |
|---|---|---|
| 父线程创建子线程 | `inherit = true`（默认继承） | architecture §4 |
| 服务端处理客户端 | **暂不启用** `_NTO_CHF_INHERIT_RUNMASK` 式继承 | `[OPEN]` 本板服务多为 systemd 拉起，无"调用方核"语义 |

---

## 11.4 核分配表

### 11.4.1 现状（`[FACT-A]`）

| 核 | 归属 | 来源 |
|---|---|---|
| **cpu0** | **视频链**（钉死，满帧 30fps） | 证据页「视频链钉 cpu0」 |
| cpu1–3 | 相机人脸、ASR、其余服务共享 | 无显式分配，靠 `nice` + 占空比 |

### 11.4.2 建议（`[PROPOSED]`，未裁定，需与各服务方协商）

| 核 | 归属 | 理由 |
|---|---|---|
| **cpu0** | 视频链（**维持现状，不动**） | 30fps 硬时序，动它风险最高 |
| **cpu1–3** | 相机人脸 + ASR + TTS 共享池 | 三者均为突发型，可错峰 |
| 其中 cpu3 | **建议留给相机人脸**（占用最稳定，约 0.7 核） | 稳定负载钉单核最省心 |

**冲突处理**：`affinity_required` 不满足时**拒绝准入并报错**，不静默降级。

> ⚠️ 本文**不**改动 cpu0 的分配。视频链的时序约束不在本文验证范围内。

---

## 11.5 实测依据

### 11.5.1 核归属的收益（TTS，2026-11-02，相机服务在跑）

| 配置 | 整链中位耗时 | 标准 RTF | 相对 |
|---|---:|---:|---:|
| 4 线程、**不绑核** | 13689 ms | 1.030（慢于实时 3%） | 基准 |
| **3 线程、绑核 0–2、保留自旋** | **11019 ms** | **0.835（快于实时 16%）** | **−19.5%** |

`[FACT-A]`：四句全量、每句 3 次取中位、一配置一独立进程。**光"核归属"一项 −19.5%，比同轮的"关自旋"（−17.3%）还大。**

### 11.5.2 实时优先级 + 绑核的尾延迟

| 配置 | max 延迟 | 证据 |
|---|---:|---|
| CFS（不绑核） | **10.803 ms** | `[FACT-A]`（记于 `baseline/07-platform-baseline.md`） |
| **FIFO + 绑核** | **1.124 ms** | 同上 |

**≈10× 尾延迟改善**。注意该页同时声明：这是特定测试负载读数，**不是硬实时保证**。

### 11.5.3 自旋等待的取舍

`[FACT-A]` 同一轮实测：

| 场景 | 该开还是该关 `allow_spinning` | 原因 |
|---|---|---|
| **已绑核（有专属核）** | **开** | 无争抢，自旋的快唤醒是净收益；绑核+自旋实测最快 |
| **未绑核（与他人共享核）** | **关**（`OMP_WAIT_POLICY=passive`） | 自旋空转抢 CPU，双方互抢；关自旋 −17.3% |
| 空载独占 | 开 | 无争抢时自旋通常更优 |

**⇒ "绑核"和"关自旋"不是二选一**：有了核归属，自旋应重新打开。这也说明**核归属是更根本的那一层**。

---

## 11.6 本板验收判据

对应 architecture §5 的验收要求，本板逐条落地：

- [ ] `ExecutionContext.affinity` 设置后，`taskset -pc <pid>` 读回与请求一致（4 核逐核验证）。
- [ ] `affinity_required` 不满足时**拒绝准入并返回明确错误**，不静默降级。
- [ ] `CPUQuota` 一类约束在本板**报 `unsupported`**——用"设了 quota 但不生效"的对照实验证明，不得报 `enforced`。
- [ ] 进程退出后核归属被回收；不回收要能报 leak。
- [ ] 视频链在 cpu0 的 30 fps 不因本设计下降（回归判据）。
- [ ] 相机人脸服务钉核后，其自身吞吐/延迟不退化（**须与服务方共同验收**）。
- [ ] FIFO 绑核的尾延迟改善在目标负载上复现（对照 §11.5.2）。

---

## 11.7 未决

| # | 事项 | 状态 |
|---|---|---|
| 1 | 相机人脸能否接受钉在单核（可能有多线程并发段） | `[OPEN]` 需与服务方共同测，**不得单方面改它的 unit** |
| 2 | `isolcpus=` 是否值得（需改内核 cmdline + 重启） | `[OPEN]` 收益未知、风险高，暂不做 |
| 3 | `SCHED_DEADLINE` 在本板的实际可用性（`chrt -d` 冒烟） | `[OPEN]` 证据页标 C |
| 4 | 核分配是否需要跨重启持久（systemd drop-in vs 运行时 API） | `[OPEN]` |
| 5 | 服务端继承调用方核（QNX `_NTO_CHF_INHERIT_RUNMASK` 式）在本板是否有实际场景 | `[OPEN]` 见 §11.3.2 |

---

## 11.8 相关

- `../../architecture/11-execution-and-resource-model.md` —— **对象模型、不变量、等级契约、继承语义、适配模板**（本文的上位规范）
- `baseline/01-architecture.md` §1.6 —— `Execution Context` / `Execution Contract` / `Resource Lease` 的对象定义
- `baseline/03-os-runtime.md` §3.3 / §3.7 —— `CpuBudget` 定义与 Linux Backend 映射
- `baseline/07-platform-baseline.md` §7.1 —— 本板 CPU / 抢占 / cgroup / RT 调度事实（A 级）
- `ZG330平台能力边界-原始调查记录.md` —— 「现役配额方式」一行（`taskset`＋`nice`＋占空比的生产实证）

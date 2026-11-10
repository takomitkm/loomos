# 12 — 资源租约与 Buffer 所有权

> **层级**：本文是 `architecture/` 的**跨机型现行版**（编号镜像 `baseline/`）。本机型的具体读数与取舍见 `platforms/<机型>/` 下的同号文件；两者冲突时以本层为准并留记录。
>
> **文档定位**：`architecture/` 是跨平台约束与接口规范。本文定义**资源怎么被让渡、持有、归还**，以及 **Buffer 在 CPU 与加速器之间怎么交接**。
> **不写**：某机型具体用哪些地址类型、哪个 sync API、实测多少——那些写在 `platforms/<机型>/`。

---

## 12.1. 为什么不能统一成一个 `quota`

`baseline/03-os-runtime.md` §3.3 已裁定：资源不应被统一成一个 `u64`。本文把该裁定固化为**跨机型固定集合**——机型只能声明"某个 kind 在我不支持"，不能自行增删语义。

---

## 12.2. Resource Kinds（集合固定，可用性由机型声明）

| Resource Kind | 语义 | 典型对象 |
|---|---|---|
| `CpuBudget` | 一段时间内可消耗的 CPU 时间 | runtime / period / deadline |
| `MemoryLimit` | 容量上限 | RSS / address-space |
| `BufferLease` | 对某块内存的生命周期与访问权 | 共享帧、DMA 缓冲 |
| `DeviceLease` | 设备访问／控制权 | 加速器、相机、外设 |
| `NpuSessionLease` | 加速器 Session / Backend 资源占用 | 推理会话 |
| `StorageQuota` | 持久存储资源 | 日志、模型缓存 |
| `NetworkBudget` | 网络连接／带宽策略 | 控制面通信 |

**硬约束**：**核亲和不在本表内**——它是执行对象属性，见 `11-execution-and-resource-model.md` §11.3.1。

---

## 12.3. Lease 语义（跨机型必须一致）

### 12.3.1 生命周期

```text
申请 → 准入 → 授予(Active) → [撤销/到期/持有者退出] → 回收(Released)
```

**不变量**：

1. **持有者死亡必须回收**，不得依赖持有者自觉释放；回收失败要能报 leak。
2. **未授予不得使用**：不得"先用后补租约"。
3. **租约到期不等于资源已归还**：从 Active 到 Released 之间必须有一个可观测的确认点。

### 12.3.2 独占 vs 共享

| 模式 | 语义 | 要求 |
|---|---|---|
| **Exclusive** | 同一时刻只有一个持有者 | 冲突必须**拒绝**，不得排队后静默降级 |
| **Shared** | 多持有者并发 | 必须声明并发度上限与一致性责任方 |

### 12.3.3 撤销

- 必须声明是**协作式**（持有者主动让出）还是**强制式**（系统单方面收回）。
- 强制式撤销必须说明：**在途操作怎么办**（等待／丢弃／标记未知）。

---

## 12.4. Buffer 所有权状态机（跨机型不变）

```text
CpuOwned → SyncForDevice → DeviceOwned → DeviceComplete → SyncForCpu → CpuOwned
```

**四条不变量**：

1. **未确认硬件完成前，不得释放 Buffer。**
2. **未执行 `SyncForCpu` 前，CPU 不得读取设备写入的内容**（读到陈旧数据是必然而非偶然）。
3. **不得假定 Cache 硬件一致性。** 平台若确实一致，必须给出实测证据；默认按不一致处理。
4. **所有权转移必须有明确的主体变更点**，不得"双方都以为自己拥有"。

**推论**：超时返回**不代表**设备已停止访问 Buffer；此时释放即为 use-after-free。

---

## 12.5. 地址域类型（分类固定，取值由机型填）

| 类型 | 含义 |
|---|---|
| `CpuVirtualAddress` | 进程虚拟地址 |
| `CpuPhysicalAddress` | CPU 物理地址 |
| `DeviceDmaAddress` | 设备 DMA 地址 |
| `NpuLogicAddress` | 加速器内部 MMU 映射后的逻辑地址 |

**硬约束**：

- **禁止用裸 `u64` 传递地址**（`baseline/01-architecture.md` §1.7 原则 4）。
- **"DMA 地址 == 物理地址"只能作为启动时校验的属性**，不得编码为跨平台前提——不同机型的 IOMMU/MMU 行为不同。

---

## 12.6. 各机型适配表（必填）

每个机型必须在自己 `platforms/<机型>/` 下给出下表，**缺任一行即视为未完成适配**：

| 项 | 要求 |
|---|---|
| **地址域实际取值** | 上方四类在本机分别是什么，以及是否相等（须给校验方法） |
| **Cache 一致性** | 硬件是否一致（**默认按不一致处理**，要推翻须给实测） |
| **Sync API** | `SyncForCpu` / `SyncForDevice` 在本机对应哪个调用 |
| **对齐要求** | 各数据路径的对齐字节数（相机／显示／DMA／权重等） |
| **Lease 可撤销性** | 协作式还是强制式；在途操作怎么处理 |
| **回收路径与判据** | 持有者退出后怎么确认资源已归还；leak 怎么报 |

---

## 12.7. 与 `baseline/` 的关系

- `baseline/03-os-runtime.md` §3.3 定义 Resource Model 的动机与对象集合；`baseline/05-inference-zg330.md` §5.9 给出 Buffer 状态机的初版。
- 本文把上述内容固化为**跨机型契约**，并补两条 baseline 未明写的：**租约回收的可观测确认点**、**地址域等值只能作启动校验**。
- 冲突以本文为准，须留记录。

---

## 12.8. 相关

- `11-execution-and-resource-model.md` —— 执行对象与核亲和（`CpuBudget` 的归属方）
- `13-failure-classification-and-recovery.md` —— 故障时租约怎么回收
- `baseline/03-os-runtime.md` §3.3 —— Resource Model 原始定义
- `baseline/05-inference-zg330.md` §5.9 —— Buffer 状态机初版

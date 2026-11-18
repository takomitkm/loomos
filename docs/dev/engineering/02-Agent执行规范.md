---
title: Rust 与 OS Agent 自动化执行规范
version: 0.1-draft
status: engineering-proposal
updated: 2026-11-09
---

# Rust 与 OS Agent 自动化执行规范

## 1. 任务目标

Agent 的任务不是机械删除 `unsafe`，而是逐步提高仓库的安全保证：

- 找出 unsafe 使用点和隐含不变量；
- 判断是否可以用安全 API 替换；
- 否则把 unsafe 收敛到最小可信实现；
- 强化调用接口的类型和状态约束；
- 为关键性质选择合适的验证方法；
- 将结果记录到可审计的 Safety Contract / Evidence Registry。

## 2. 强制规则

1. 不理解安全义务，不得修改对应 unsafe。
2. 不得通过删掉 `unsafe` 关键字来把不安全函数改成安全函数。
3. 不得凭猜测生成 `SAFETY:` 注释。
4. 不得删除断言、边界检查、测试或错误路径来使构建通过。
5. 不得擅自改变外部可观察行为、ABI、权限、错误码和资源生命周期。
6. 不得将 FFI、MMIO、DMA、汇编伪装成普通安全 Rust 操作。
7. 不得宣称 Miri、单元测试或 fuzz 通过就证明 soundness。
8. `Send`/`Sync`、`UnsafeCell`、`MaybeUninit`、裸指针、生命周期擦除、FFI 和 Drop/泄漏路径必须提升风险等级。
9. 不得假设 `Drop` 一定执行；必须考虑 `mem::forget`、进程崩溃、panic、断电和设备失联。
10. 任何无法验证的假设必须明确列为 `Assumed` 或 `Unverified`。

## 3. 工作流程

### Stage A — Inventory

输出：
- Rust crate、target、feature、toolchain、依赖版本；
- unsafe block / unsafe fn / unsafe impl；
- FFI、汇编、裸指针、原始内存分配；
- Send/Sync、内部可变性、无锁代码；
- 外部依赖中的 unsafe 暴露面；
- 现有测试和性能基线。

只盘点，不修改。

### Stage B — Obligation Extraction

对每个高风险位置列出：
- 操作本身；
- 所有调用路径和调用者；
- 指针有效性、对齐、初始化、边界；
- 别名、生命周期、线程安全和同步关系；
- 资源所有权和状态转换；
- 平台、固件、硬件和 FFI 假设；
- panic、错误、超时、取消和恢复路径。

找不到依据的安全条件不得臆造。

### Stage C — Refactoring Plan

按优先级评估：
1. 标准库安全 API；
2. 成熟且可审计的安全抽象库；
3. 项目内部安全包装；
4. 保留最小 unsafe 并为其建立证明义务。

给出行为差异、性能风险、兼容性影响和验证计划。高风险变更先提交计划，不要直接批量重写。

### Stage D — Implementation

- 把公共接口改为安全 API；
- 使用 Newtype、Typestate、Capability、私有构造器；
- 把状态转移做成显式操作；
- 减少裸指针在模块间传播；
- 将平台特定代码放在独立 backend；
- 将 unsafe 块尽可能缩小并贴近不变量；
- 保持错误与失败语义明确。

### Stage E — Verification

先写明目标性质，再选择工具：
- Rust compiler / Clippy：类型和 lint；
- cargo-geiger：unsafe 盘点；
- Miri：解释执行路径上的 UB 检查；
- Kani：边界明确的模型检查；
- Loom：并发交错；
- Verus：适用子集内的功能规格证明；
- Flux：适用类型与数值精化约束；
- Gillian-Rust / RefinedRust / Iris：对适用代码开展研究性验证。

工具不支持的特性、未覆盖的路径和模型假设必须记录。

### Stage F — Independent Review

Reviewer 核对：
- unsafe 义务是否完整；
- 安全 API 是否真的 sound；
- 类型约束有没有绕过路径；
- 证明规格是否足够强，是否只是证明了错误/空洞规格；
- 测试是否覆盖边界和失败路径；
- 平台假设是否被遗漏；
- 是否引入新的 unsafe 或新依赖风险；
- 性能变化是否可接受。

### Stage G — Evidence and Merge

保存：
- commit / diff；
- 契约版本；
- 工具和版本；
- 目标平台与配置；
- 测试、模型检查、证明输出；
- Reviewer 结论；
- 残余假设和未解决事项。

只有达到仓库定义的验收门槛才能合并。

## 4. 处理结果分类

每个 unsafe site 必须归入以下之一：

- `Replaced`：已由安全 API 替换；
- `Encapsulated`：仍有 unsafe，但被私有安全抽象封装；
- `Verified`：unsafe 保留且指定证明义务已经机器检查；
- `Assumption-Bounded`：依赖硬件、固件或外部代码假设，边界已记录并限制；
- `Unresolved`：没有充分证据，不得标记完成。

## 5. 风险分级

| 等级 | 示例 | Agent 自动权限 |
|---|---|---|
| Low | 安全 Rust 小函数、纯数据转换 | 可直接修改并运行测试 |
| Medium | 容器逻辑、状态机、有限裸指针封装 | 先生成计划，可在测试后提交审查 |
| High | unsafe impl Send/Sync、并发原语、分配器、页表 | 必须建立契约和独立审查 |
| Critical | DMA、MMIO、汇编、中断入口、FFI ABI、设备复位 | 只能提出方案和辅助证明；必须由指定审查流程批准 |

## 6. 每次任务的报告格式

```text
Change ID:
Unsafe Site(s):
Original Behavior:
Callers:
Safety Obligations:
Invariant Sources:
Proposed Abstraction:
Type-Level Guarantees:
Runtime Enforcement:
Verification Methods:
Verification Results:
Tool Versions / Target:
Residual Assumptions:
Remaining Unsafe:
Behavior / ABI Changes:
Performance Impact:
Independent Review:
Disposition: Replaced | Encapsulated | Verified | Assumption-Bounded | Unresolved
```

## 7. Definition of Done

一个任务只有在以下条件满足时完成：

- 原 unsafe 义务已消除、封装、证明或明确保留；
- 新的安全 API 没有引入新的 soundness 问题；
- 关键不变量有来源且可以复核；
- 适用的测试/验证通过；
- 证明边界和未验证假设有记录；
- 独立审查完成；
- 证据关联到具体变更和目标平台。

“代码编译成功”“测试全部通过”“unsafe 数量减少”均不能单独作为完成标准。

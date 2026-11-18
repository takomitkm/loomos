# AGENTS.md — loomos 仓库 Agent 执行规范

> 本文件是 Agent 在本仓库工作的**最高行为规范**。它合并了
> `docs/dev/engineering/02-Agent执行规范.md`（Rust/unsafe 纪律）与
> `docs/dev/engineering/templates/安全契约模板.md`（证据格式）。
> 冲突时以本文件为准；对架构本身有疑问先读 `docs/dev/architecture/`。

## 0. 30 秒结论

- **改任何代码前后都要跑** `bash agent/run-prototype.sh`（全绿=退出码 0）。
- unsafe 默认禁止；本仓库当前 0 unsafe，保持它。
- 不确定的事**停下写进 OPEN**，不要猜、不要编造证据。

## 1. 一键执行（Agent 的第一入口）

```bash
bash agent/run-prototype.sh          # 全流程：test → fixtures → smoke → 报告
bash agent/run-prototype.sh --fast   # 跳过已通过的测试（仅快速 smoke）
```

- 退出码 0 = 全绿；非 0 = 有失败，**先看 `reports/prototype-report.md` 再改代码**。
- 报告是本仓库的验收正本；不要手工编辑报告，让它由脚本生成。
- 任何架构性修改（新 crate、改状态机、改契约）必须让 smoke 的场景表保持 12/12 PASS，
  或在 `crates/loomos-cli` 的 `expectations()` 中**显式更新期望并说明原因**。

## 2. 强制规则（违者变更必须回滚）

1. 不理解安全义务，不得修改对应 unsafe（本仓库目标是保持 0 unsafe）。
2. 不得删除断言、边界检查、测试或错误路径来让构建/测试通过。
3. 不得凭猜测生成 `SAFETY:` 注释、测试结果或证明结论。
4. 不得把「测试通过」「编译通过」写成 soundness / 证明——证据分级见 §4。
5. 不得假设 `Drop` 一定执行：涉及资源回收的协议必须有内核侧兜底
   （参考 `loomos-runtime::resource::LeaseManager` 的 Orphaned/清扫设计）。
6. 实施等级如实上报：`enforced` 必须有设置后读回证据；做不到就写
   `best_effort(失效条件)` 或 `unsupported`（architecture/11 §11.3.2）。
7. `Unknown` 不得映射成 `Failed`（设备/推理操作）；超时不等于设备已停止访问 Buffer。
8. 平台能力（cgroup/seccomp/IOMMU…）不得假设存在——先查
   `docs/dev/platforms/FMQL30TAI676H/07-platform-baseline.md` 的证据等级。
9. 新依赖必须过依赖准入（architecture/08 §8.2–8.3）：许可证/MSRV/依赖树/
   x86 测试/交叉编译检查，全部记录后才能进 Cargo.toml。
10. 文档纪律：baseline 只读；跨机型约束进 `docs/dev/architecture/`；
    板级事实进 `docs/dev/platforms/<机型>/`；一个事实只写一处（见 llms.txt）。

## 3. 本仓库的架构不变量（改代码前先确认不破坏）

| 不变量 | 位置 | 验证测试 |
|---|---|---|
| 租约独占冲突必须拒绝（不静默排队） | runtime/resource | `exclusive_conflict_is_rejected_not_queued` |
| Active→Released 有可观测确认点（Releasing） | runtime/resource | `release_requires_observable_confirm_point` |
| 持有者死亡 → Orphaned → 清扫 | runtime/resource | `holder_death_orphans_then_sweeps` |
| 子 namespace 授权不自动扩大到父级 | runtime/capability | `namespace_scope_does_not_expand_upward` |
| 子集委托、越权拒绝 | runtime/capability | `delegation_cannot_escalate` |
| Namespace 路径拒绝 `..`/空段/逃逸 | runtime/config | `namespace_path_validation_rejects_escape` |
| Revision 冲突可检测；Outbox 只在提交后发布 | runtime/config | `revision_conflict_detected` 等 |
| Desired ≠ Applied；失败保留差异不伪造成功 | runtime/config | `desired_applied_flow` |
| Buffer 未确认完成不得释放（typestate + 隔离区） | inference/buffer | `dropping_device_owned_panics_in_debug` 等 |
| 排队可取消；执行中取消 → `Unknown` 不谎报 | inference/orchestrator | `cancel_only_before_start` 等 |
| Operation `Unknown` 不映射 `Failed` | device/core | `hang_returns_unknown_not_failed` |
| 直连表只整句匹配；「不要开始录制」不命中 | nlu/grammar | `direct_table_exact_match_only` |
| 危险动作确认用过即废、绑定动作+参数+主体+TTL | home/confirm | `consume_once_then_gone` 等 |
| 四道闸任一不过不开门 | home/door | `enforce_mode_blocks_uncalibrated_pad` 等 |
| 统一 `catch→restart()` 禁止；按四类故障分级恢复 | runtime/supervisor | `device_hang_escalates_when_reset_unconfirmed` 等 |

新增/修改状态机时：先写状态转换表，再写实现，再补「非法转换被拒绝」的负测试。

## 4. 证据分级（报告中如实标注）

`TypeChecked`（Rust 类型系统）· `Linted` · `Tested`（测试/端到端回放）·
`Explored`（模型检查，未做）· `Proved`（机器检查证明，未做）·
`HardwareEnforced`（需板测）· `Assumed` · `Unverified`。

**本仓库当前全部证据只到 Tested/TypeChecked。** 引用旧板实测数据时必须写
「旧板读数，来源 EVIDENCE.md」，不得当成本机结论。

## 5. 常见任务路径

| 任务 | 做法 |
|---|---|
| 新增一条语音指令 | `loomos-nlu` 加意图/短语（若直连，加进 `direct_table`；语法类加规则）→ `loomos-home::ActionRegistry` 加映射 → `loomos-cli::expectations()` 加期望 → 加夹具（`standard_fixtures()`）→ smoke 验证 |
| 新增一种设备 | 实现 `DeviceProvider`（mock 先行）→ 注册进 `loomosd::System::assemble` → Capability 授权 → （可选）HTTP 适配器 + 板端契约测试 |
| 换真实引擎（sherpa-onnx / klar / 厂商 NPU） | 实现对应 trait（`AsrBackend`/`TtsBackend`/`KwsBackend`/`VadBackend`/`Backend`/`ExternalNlu`），feature-gate，**默认档保持 echo/sine 全自检** |
| unsafe 重构任务 | 读 `agent/tasks/unsafe-refactor.md`，按其中流程与停止条件执行 |
| 配置变更 | 走 Configuration Service（Schema 校验 + Revision + Outbox），不要旁路读文件 |

## 6. 报告格式（高风险变更必须带）

```text
Change ID:
Affected Sites:
Original Behavior:
Safety Obligations:
Proposed Abstraction:
Type-Level Guarantees:
Runtime Enforcement:
Verification: （测试名列表 + smoke 结果）
Evidence Level: TypeChecked | Tested | ...
Residual Assumptions:
Behavior Changes:
Disposition: Replaced | Encapsulated | Verified | Assumption-Bounded | Unresolved
```

## 7. 停止条件（遇到就停下报告，不要猜）

- 硬件/厂商文档不足（NPU/固件行为未知）；
- FFI 所有权/生命周期不明确；
- 并发不变量无法重建；
- 安全替换会改变可观察语义；
- 超时/取消路径无法证明设备已停止访问内存；
- 任何需要伪造证据才能「通过」的场景。

## 8. 记录规则（从 architecture/10 §10.5 继承）

1. 事实、推断、建议、裁定分开写。
2. 不把仿真/间接估算/宿主机读数标成板上实测。
3. 已闭合事项从待办移除，不要反复重验。
4. 高风险板测（L2/L3）不进无人值守 CI。

# 0001 — 已否决：把 LoomOS 安全权威迁入 Wasm 的交接稿片段

- **归档日期**：2026-11-22
- **状态**：Superseded（被 ADR-011 v1.1 / ADR-012 取代）
- **来源**：早期 Agent 交接稿的 Wasm 迁移方案 + ADR-011 v1.0 架构图（已被 v1.1 修正）

## 错在哪

该方案把下列 LoomOS 权威组件列为「可迁移进 Wasm Guest」的业务组件：

> Wasm Components（统一 wasm32-wasip1 编译目标）
>  ├── loomos-runtime 可部署部分（config / capability / lifecycle）   ← 错误
>  ├── loomooms-nlu / loomooms-home / loomooms-executor / …（业务逻辑）

即：**CapabilityStore（能力授权）、ResourceLease（资源租约）、Supervisor（故障恢复）、
LifecycleManager（生命周期权威状态）、Configuration Service（权威配置）被规划为
Wasm Guest 侧代码**，并表述为「config / capability / lifecycle 的可部署部分」。

## 为什么错

1. **Guest 不能成为给自己授权的主体**。能力检查、租约所有权、Supervisor 恢复决策
   必须运行在 Guest 沙箱之外；放进去等于让被管对象持有管理权，信任边界自洽性崩塌。
2. **租约/Buffer 所有权依赖 Native 数据面**。Buffer 的 DMA 地址、缓存同步、设备会话
   都在 Native Host 手里；租约裁决放 Guest 侧只能拿到句柄影子，裁决与事实脱节。
3. **Supervisor 需要重启 Guest**。被监管者不能同时是监管者——Guest 崩溃后无法
   自我恢复，恢复决策必须留在 Native。
4. **与 ADR-011 自己的裁定冲突**。ADR-011 v1.0 第 4 条已写明「Native Host 负责平台
   适配」；架构图却把权威组件列进 Wasm 迁移清单，自相矛盾。

## 被什么取代

| 错误片段 | 现行归属 |
|---|---|
| capability / lifecycle 迁入 Wasm | **Native Host 权威**：`loomos-runtime`（CapabilityStore / LeaseManager / Supervisor / component 生命周期）永不 Wasm 化 |
| config 权威迁入 Wasm | Configuration Service 权威留 Native；Guest 只经 `loomos.config_get_v1` 读（见 ADR-011 v1.1） |
| 「AOT ≈ Native」普适性能表述 | 仅 loomos-nlu 已测负载成立（P50 1.2µs vs 400ns），不外推（ADR-011 v1.1 §后果） |
| 命名 `loomooms-*` | 统一 `loomos-*`；crate 目录 `crates/loomos-wamr`（原 `loomooms-wamr`） |
| raw WAMR FFI 放在业务包装 crate 内 | 独立第三可信边界 crate `loomos-wamr-sys`（与 `loomos-sys` / `loomos-ort-sys` 同地位），`loomos-wamr` 恢复 `#![forbid(unsafe_code)]` |

系统职责的最终三分（WAMR=执行 / Nix=重建 / Native Host=权威）见
`docs/dev/engineering/ADR-012-Nix系统重建与包管理权威.md` 与
`docs/dev/architecture/14-nix-system-reconstruction.md`。

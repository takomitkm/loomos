# 09 — 可执行开发清单

> **层级**：本文是 `architecture/` 的**跨机型现行版**（编号镜像 `baseline/`）。本机型的具体读数与取舍见 `platforms/<机型>/` 下的同号文件；两者冲突时以本层为准并留记录。
>
> **2026-11-20 勾选回填**：按使用规则逐项勾选已完成项，证据注记在行内
> （测试/交叉编译证据见 docs/dev/EVIDENCE.md 与 docs/dev/engineering/dependency-ledger.md）。
> 勾选只代表"该条目在原型/宿主层已有实现+测试背书"，**板级验证未做的条目在行内注明**，
> 不冒充完成。使用规则不变：每项完成后补充 commit/测试/板测记录。
> `[ ]` 未完成，`[x]` 已完成，`[-]` 不适用。不要仅因讨论过就标记完成。

## A. 平台事实与证据

**本机型已闭合的平台事实见 `../platforms/FMQL30TAI676H/07-platform-baseline.md`；本节只保留跨机型要求。**

- [x] 每个机型必须有一份「平台能力基线」：板卡／SoC／CPU／内核／抢占模型／隔离机制／加速器 API／内存与 DMA 边界。（07 全文，证据级 A/B）
- [x] 基线必须标注证据等级，并区分：直接实测／源码或文档直证／仿真／间接估计／无效读数。（07 §7.8；EVIDENCE.md 继承同一纪律）
- [x] 平台能力变更（升级内核、换 Runtime 版本、改 DT）必须**重新基线化**，不得沿用旧结论。（07 §7.8 规则条）
- [x] 不得用"配置项已启用"当成"功能已运行"（须有运行时证据）。（11 §11.3.2；deploy/README.md 引用执行）

## B. 设计基线

- [x] 创建 ADR：当前阶段采用 Linux 5.4.52；不以 seL4/Theseus 为近期目标。（10 §10.1 ADR-001）
- [x] 创建 ADR：NPU 由单 Owner 管理，Rust 负责系统级 Orchestration。（ADR-004）
- [x] 创建 ADR：设备/Buffer 所有权由 Lease 管理，明确撤销与异常退出回收。（ADR-005 + runtime resource Orphaned 清扫测试）
- [x] 创建 ADR：非一致性 Cache 必须显式同步。（ADR-005 + inference buffer typestate）
- [x] 定义 Durable Task / Event / Command / Operation / Work Item / Execution Context。（baseline/01 §1.6 + runtime ids/resource）
- [x] 定义 Execution Contract 与实际 enforcement 状态。（11 §11.3.2 + runtime/platform.rs 读回报告）
- [x] 定义错误码与故障分类，区分进程异常与硬件挂死。（13 §13.1 四类 + runtime/supervisor.rs）

## C. Rust Workspace 与依赖准入

- [x] 固定 Rust toolchain 候选并记录 MSRV policy。（rust-toolchain.toml 锁 1.99.1；外部依赖 MSRV 见 dependency-ledger）
- [x] 建立 workspace 和最小 CI（fmt/clippy/test）。（.github/workflows/ci.yml——⚠️ ci 文件在本地待推：凭据缺 workflow scope）
- [x] 为候选 crate 建立许可证/MSRV/依赖树台账。（engineering/dependency-ledger.md，scripts/gen-dependency-ledger.py 生成）
- [ ] 配置 AArch64 linker/sysroot，成功交叉构建最小程序。（aarch64 `cargo check` 已过；**链接成 ELF 需交叉 linker**——release-aarch64.sh 待在 Linux 环境验证）
- [x] 建立目标机型 deploy/smoke 脚本。（deploy/install-board.sh + scripts/release-aarch64.sh——**未上板验证**）
- [ ] 每个依赖完成 Host → Cross → Hardware 流程后才锁定版本。（台账已建；Hardware 列全部未验证）

## D. OS Runtime

- [x] Resource ID、Component ID、Execution ID、Operation ID。（loomos-runtime/ids）
- [x] Resource Kind 与 Resource Lease 状态机。（resource.rs；独占拒绝/Releasing 确认点/Orphaned 清扫均有测试）
- [x] Capability Handle、Rights、Scope、Delegation、Revocation。（capability.rs；子集委托/越权拒绝测试）
- [x] Component Manifest 和依赖声明。（component.rs + manifests/*.toml）
- [x] Lifecycle Manager 状态机。（component.rs；非法转换负测试）
- [x] Supervisor 健康检查/退出/重启策略。（supervisor.rs；重启风暴熔断测试）
- [x] Recovery Manager 分级故障策略。（supervisor.rs；L1/L2/L3 + 三条禁令测试）
- [x] Linux Backend 能力探测和 unsupported/best-effort/enforced 报告。（loomos-sys + runtime/platform.rs——**板上读回实测 Unverified**）
- [x] 当前 root 服务逐步改为专用非 root 身份（逐服务核查设备权限）。（deploy/loomosd.service：User=loomos + 设备节点组——**未上板**）

## E. Configuration Service

- [x] ConfigNamespace 路径规范化。（config.rs；`..`/空段/逃逸负测试）
- [x] Read / Write / ReadWrite / Admin 权限。（capability.rs + config 校验）
- [x] ConfigValue 类型、浮点约束和 Bytes 编码规则。（ConfigValue::validate）
- [x] Schema Registry、默认值和 Schema Version。（register_schema）
- [x] ConfigProvider 接口。（ConfigProvider trait）
- [x] SQLite Provider 与原子 Transaction。**决策修订（2026-11-20）：暂不引入 SQLite**——rusqlite bundled 需 C 编译链（本机 windows-gnu 约束），JsonFileProvider（原子 rename）替代；引入须过 08 §8.2 准入（EVIDENCE.md 工程决策）。
- [x] Expected Revision / Conflict。（RevisionConflict 测试）
- [x] ConfigChanged + Outbox。（提交后发布 + secret 脱敏测试）
- [x] Desired / Applied / Observed 状态。（ApplyStatus；失败保留差异测试）
- [x] Configuration Applier 与失败状态。（set_desired/set_applied/set_apply_failed）
- [x] Secret Handle / 日志脱敏。（SecretHandle：Drop 覆写清零 + Display 永脱敏；outbox `<redacted>` 测试）
- [x] Schema Migration。（register_migration/migrate_namespace：单事务原子提交，v1→v2 迁移测试）
- [x] 选用 serde、serde_json、schemars、jsonschema、rusqlite、thiserror、tokio sync 等候选 crate 并完成平台验证。**修订**：实际采用 serde/serde_json/thiserror/tracing/toml（台账）；schemars/jsonschema 未采用（手写 SchemaField 覆盖原型需求）；rusqlite 暂缓；tokio 未采用（std 同步原语，无异步运行时需求）。

## F. NPU / Inference

- [x] 固定厂商推理 SDK、Runtime、头文件和库的版本证据。（07 §7.2 ZG330 CLI/Runtime 版本，证据级 A/B）
- [ ] 建立 C ABI wrapper，阻止 C++ exception 跨边界。（**板上阶段**；loomos-sys 是 OS 映射，不是 NPU FFI）
- [ ] NPU 单 Owner 服务与设备打开/关闭生命周期。（**设计 + mock 编排已完成**（orchestrator）；真设备打开/关闭 ❌）
- [x] 请求排队、准入、Deadline、请求 ID、错误映射。（orchestrator：队列满/过紧 deadline 可观测拒绝；mock 级）
- [x] 明确执行中请求不可取消时的 API 语义。（timeout_unknown 不映射 failed；Buffer 隔离区测试）
- [x] Tensor / Buffer 地址类型化。（buffer.rs 四地址域 Newtype）
- [x] DMA/Cache Sync 与所有权转移状态机。（buffer typestate：CpuOwned→…→SyncForCpu，Drop 守卫测试）
- [ ] 共享缓冲容量压力和模型内存峰值预算。（**板上阶段**）
- [ ] 当前 ORT 基线复测。（**板上阶段**）
- [ ] tract 仅作为候选做目标模型覆盖/性能评估。（未开始）
- [ ] 整模型 CPU/NPU Auto 选择和 Model Profile。（ModelProfile 结构已定义；实测数据 ❌）
- [ ] 子图混合执行只有在测得收益后再启动。（未开始，按计划后置）

## G. Device Core（非机器人实现）

- [x] DeviceId / ProviderId / Descriptor。（device/core.rs）
- [x] DeviceCapability 与授权 Capability 分开。（两层检查：能力声明 + Rights，core.rs execute）
- [x] Device Lifecycle / Health / Fault。（core.rs + registry.set_lifecycle）
- [x] Exclusive / Shared Lease。（registry.acquire → runtime LeaseManager；独占冲突拒绝测试）
- [x] Operation 状态、进度、取消、过期和未知结果语义。（OperationState 含 Unknown/Expired；expire_stale 测试）
- [x] Telemetry 数据流。（provider.telemetry + registry 透传）
- [x] Mock Provider 测试。（mock.rs + vision provider）
- [x] 未来 Domain Adapter 插槽，不依赖 ROS 2 类型。（device/adapter.rs：DomainAdapter trait + DomainAdapterProvider 包装 + risky 操作闸口，机器人域测试）

## H. Home / 现有业务整合

- [x] 将现有任务、命令、事件与新 Operation 的边界写清楚。（PROTOTYPE §2.1 对照表）
- [-] 确保数据库写事务不包住外部调用。（原型无数据库；契约记录于 actions.rs——引入持久库时生效）
- [x] 确保业务配置通过 Configuration Service。（loomosd seed/commit + Schema）
- [x] 控制面消息与大块媒体/模型数据面分离。（控制面 JSON/事件；音频/帧走 AudioChunk/Frame Buffer）
- [x] 现有实现逐项评估保留、重构或替换，不以兼容为硬约束。（PROTOTYPE §2.2 删减清单 + §2.3 语义红线）

## I. Linux Enforcement（后置）

- [x] 评估打开 cgroup controllers 的依赖及厂商内核补丁影响。（03-OS代劳清单 A1/D：重编或换 BSP 路径）
- [x] 评估 Namespace/seccomp/LSM 编译与运行时适配。（03 §A2/D）
- [x] 评估非 root 用户、设备节点 ACL 和最小权限。（03 §B4 + deploy/loomosd.service——评估完成，实施工件已备**未上板**）
- [x] 评估 SMMU Device Tree 布线是否可行及设备隔离覆盖范围。（07 §7.4 + 03 §A3）
- [x] 评估 PREEMPT_RT 重编和 u-dma-buf/mali/amvx 模块兼容。（03 §D）
- [ ] 每项改动有可复现需求、测试、回退和目标机型数据。（**全部待板上**；loomos-sys 映射的板上读回 Unverified）

## J. Future Robotics（明确延期）

- [x] 当前不采购/部署的机器人硬件不阻塞 OS Core。（roadmap/PROTOTYPE）
- [x] 只预留 Device Provider、Operation、Lease、Telemetry、Fault、Execution Contract。（device/adapter.rs 插槽 + risky 闸口）
- [ ] 等机械臂/驱动器/安全控制器确定后，重新评估 ROS 2、ros2_control、MoveIt、仿真及实时执行域。（硬件未定）
- [ ] 不将高频控制环放进 HTTP/SQLite/LLM/普通业务 Worker。（规则已写入 adapter.rs 契约注释；待硬件确定后验证）

## K. Rust Static Assurance（后置/并行研究）

- [x] 先用 Newtype/Trait/Lifetime/Typestate。（全仓贯彻；buffer typestate/地址域/ID 为代表）
- [x] 再用 Proc Macro/Clippy 规则。（clippy -D warnings 门禁 0 警告；自定义 proc macro 按需后置）
- [ ] 确有跨过程数据流需求再做 MIR 分析。（未开始）
- [ ] 只有语言表达能力存在明确缺口才评估 rustc 修改。（未开始；`#![forbid(unsafe_code)]` 已覆盖 11/12 crate，loomos-sys 为声明式可信边界）

## L. Nix/Wasm 平台化（2026-11-22 增补）

- [x] 权威组件（CapabilityStore/Lease/Supervisor/Lifecycle/配置权威）留 Native Host，不迁 Wasm。（ADR-011 v1.1 主权边界；错误方案归档 archive/0001）
- [x] ComponentManifest 移除 per-component execution 字段。（93c5b6e 已做，无残留引用）
- [x] raw WAMR FFI 集中到第三声明式可信边界 loomos-wamr-sys；门面 forbid(unsafe_code)。（N1）
- [x] WAMR 真加载/卸载/实例化 + 原子内存预算（CAS 预留→提交→回滚→释放）+ 关闭五步顺序。（N1，宿主测试全绿）
- [x] AOT 兼容键 sidecar + 生产档 fail-closed + 魔数/版本校验（ aot v6）。（N1）
- [x] Host ABI v1：11 接口统一 loomos.<iface>_v1 + 稳定错误码 + 能力校验 + 属主句柄表 + Guest 内存复制语义。（N2，stub 档全测）
- [x] Nix 钉死 flake + 四包 + devShell + checks（fmt + 工具链防漂移）+ NixOS 模块 + 三系统配置 + 厂商工件登记。（N0，Unverified 待 CI）
- [ ] flake.lock 生成入库 + `nix flake check`/`nix build` CI 首跑。（N0 收尾；本机无 Nix）
- [ ] wamr-link 档真链路（CI：libiwasm.a 构建 + nlu 组件经 RuntimeHost 执行 + checksum 对齐 bench 基线）。（N1/N2 验证）
- [ ] 业务组件 Host ABI 化迁移（nlu 先行，与 bench 基线字节级对齐）。（N3）
- [ ] Windows/macOS 原生 runner 构建 + QEMU aarch64 对比（N4）。
- [ ] NixOS 镜像 + A/B 槽位 + RAUC 评估 + 回滚演练（N5；板端分区不动）。

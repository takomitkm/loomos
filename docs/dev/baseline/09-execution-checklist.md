# 09 — 可执行开发清单

> 使用规则：每项完成后补充 commit/测试/板测记录。`[ ]` 未完成，`[x]` 已完成，`[-]` 不适用。不要仅因讨论过就标记完成。

## A. 平台事实与证据

- [x] 板卡/SoC/CPU/Ubuntu/Kernel 基线已记录。
- [x] cgroups/Namespace/seccomp/LSM 当前缺失已确认。
- [x] FIFO/RR/DEADLINE 当前可用性已板测。
- [x] ICraft 3.39.0 API 面、无系统 Scheduler、forward 同步阻塞已记录。
- [x] NPU hang 需要断电的恢复边界已记录。
- [x] udmabuf 128 MiB、PS/PL DDR、非一致 Cache、无运行时 IOMMU 已记录。
- [x] CacheSync 0.088 µs/op 的测量口径已注明。
- [x] PL AXI DMA 72.3/72.9 MB/s 与 PS memcpy/NPU profiler 估计已区分。
- [ ] 将本次板测原始命令/输出归档到可追溯位置（如现有 Wiki 已有正本，添加链接）。
- [ ] 下次安全窗口确认 `Device::reset(level)` 的厂商语义；确认前不用于恢复。
- [ ] 如有实际性能决策需要，再安排 NPU 前向专用测量；不得用不支持模型或间接 profiler 数字冒充直接带宽。

## B. 设计基线

- [ ] 创建 ADR：当前阶段采用 Linux 5.4.52；不以 seL4/Theseus 为近期目标。
- [ ] 创建 ADR：NPU 由单 Owner 管理，Rust 负责系统级 Orchestration。
- [ ] 创建 ADR：设备/Buffer 所有权由 Lease 管理，明确撤销与异常退出回收。
- [ ] 创建 ADR：非一致性 Cache 必须显式同步。
- [ ] 定义 Durable Task / Event / Command / Operation / Work Item / Execution Context。
- [ ] 定义 Execution Contract 与实际 enforcement 状态。
- [ ] 定义错误码与故障分类，区分进程异常与硬件挂死。

## C. Rust Workspace 与依赖准入

- [ ] 固定 Rust toolchain 候选并记录 MSRV policy。
- [ ] 建立 workspace 和最小 CI（fmt/clippy/test）。
- [ ] 为候选 crate 建立许可证/MSRV/依赖树台账。
- [ ] 配置 AArch64 linker/sysroot，成功交叉构建最小程序。
- [ ] 建立板端 deploy/smoke 脚本。
- [ ] 每个依赖完成 Host → Cross → Hardware 流程后才锁定版本。

## D. OS Runtime

- [ ] Resource ID、Component ID、Execution ID、Operation ID。
- [ ] Resource Kind 与 Resource Lease 状态机。
- [ ] Capability Handle、Rights、Scope、Delegation、Revocation。
- [ ] Component Manifest 和依赖声明。
- [ ] Lifecycle Manager 状态机。
- [ ] Supervisor 健康检查/退出/重启策略。
- [ ] Recovery Manager 分级故障策略。
- [ ] Linux Backend 能力探测和 unsupported/best-effort/enforced 报告。
- [ ] 当前 root 服务逐步改为专用非 root 身份（逐服务核查设备权限）。

## E. Configuration Service

- [ ] ConfigNamespace 路径规范化。
- [ ] Read / Write / ReadWrite / Admin 权限。
- [ ] ConfigValue 类型、浮点约束和 Bytes 编码规则。
- [ ] Schema Registry、默认值和 Schema Version。
- [ ] ConfigProvider 接口。
- [ ] SQLite Provider 与原子 Transaction。
- [ ] Expected Revision / Conflict。
- [ ] ConfigChanged + Outbox。
- [ ] Desired / Applied / Observed 状态。
- [ ] Configuration Applier 与失败状态。
- [ ] Secret Handle / 日志脱敏。
- [ ] Schema Migration。
- [ ] 选用 serde、serde_json、schemars、jsonschema、rusqlite、thiserror、tokio sync 等候选 crate 并完成平台验证。

## F. NPU / Inference

- [ ] 固定 ICraft 3.39 SDK、Runtime、头文件和库的版本证据。
- [ ] 建立 C ABI wrapper，阻止 C++ exception 跨边界。
- [ ] NPU 单 Owner 服务与设备打开/关闭生命周期。
- [ ] 请求排队、准入、Deadline、请求 ID、错误映射。
- [ ] 明确执行中请求不可取消时的 API 语义。
- [ ] Tensor / Buffer 地址类型化。
- [ ] DMA/Cache Sync 与所有权转移状态机。
- [ ] udmabuf 容量压力和模型内存峰值预算。
- [ ] 当前 ORT 基线复测。
- [ ] tract 仅作为候选做目标模型覆盖/性能评估。
- [ ] 整模型 CPU/NPU Auto 选择和 Model Profile。
- [ ] 子图混合执行只有在测得收益后再启动。

## G. Device Core（非机器人实现）

- [ ] DeviceId / ProviderId / Descriptor。
- [ ] DeviceCapability 与授权 Capability 分开。
- [ ] Device Lifecycle / Health / Fault。
- [ ] Exclusive / Shared Lease。
- [ ] Operation 状态、进度、取消、过期和未知结果语义。
- [ ] Telemetry 数据流。
- [ ] Mock Provider 测试。
- [ ] 未来 Domain Adapter 插槽，不依赖 ROS 2 类型。

## H. Home / 现有业务整合

- [ ] 将现有任务、命令、事件与新 Operation 的边界写清楚。
- [ ] 确保数据库写事务不包住外部阻塞调用。
- [ ] 确保业务配置通过 Configuration Service。
- [ ] 控制面消息与大块媒体/模型数据面分离。
- [ ] 现有实现逐项评估保留、重构或替换，不以兼容为硬约束。

## I. Linux Enforcement（后置）

- [ ] 评估打开 cgroup controllers 的依赖及厂商内核补丁影响。
- [ ] 评估 Namespace/seccomp/LSM 编译与运行时适配。
- [ ] 评估非 root 用户、设备节点 ACL 和最小权限。
- [ ] 评估 SMMU Device Tree 布线是否可行及设备隔离覆盖范围。
- [ ] 评估 PREEMPT_RT 重编和 u-dma-buf/mali/amvx 模块兼容。
- [ ] 每项改动有可复现需求、测试、回退和板端数据。

## J. Future Robotics（明确延期）

- [ ] 当前不采购/部署的机器人硬件不阻塞 OS Core。
- [ ] 只预留 Device Provider、Operation、Lease、Telemetry、Fault、Execution Contract。
- [ ] 等机械臂/驱动器/安全控制器确定后，重新评估 ROS 2、ros2_control、MoveIt、仿真及实时执行域。
- [ ] 不将高频控制环放进 HTTP/SQLite/LLM/普通业务 Worker。

## K. Rust Static Assurance（后置/并行研究）

- [ ] 先用 Newtype/Trait/Lifetime/Typestate。
- [ ] 再用 Proc Macro/Clippy 规则。
- [ ] 确有跨过程数据流需求再做 MIR 分析。
- [ ] 只有语言表达能力存在明确缺口才评估 rustc 修改。

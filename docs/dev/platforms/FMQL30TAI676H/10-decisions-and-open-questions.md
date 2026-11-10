# 10 — 本板开放事项

> **层级**：通用决策模板（ADR 格式、技术替换记录模板、记录纪律）见 `../../architecture/10-decisions-and-open-questions.md`。
> **本文只列本机型仍未闭合的事项**；通用 ADR-001～010 不在此重复。

---

## 10.3 本板 OPEN

| ID | 事项 | 是否阻塞当前主线 | 处理方式 |
|---|---|---|---|
| OPEN-01 | `Device::reset(level)` 厂商语义 | 不阻塞一般 OS Core；阻塞将其用作恢复策略 | 向厂商确认；确认前不使用 |
| OPEN-02 | NPU 前向干净直接带宽 | 不阻塞单 Owner/Orchestration 设计 | 仅对模型/算子支持的有效模型，在受控窗口测量 |
| OPEN-03 | PS↔PL↔NPU 完整数据路径带宽 | 不阻塞 Buffer API 初版 | 明确数据路径后分段测量，避免混用 DMA memcpy 指标 |
| OPEN-04 | CPU DW DMA 单通道数据 | 当前现役主要链不依赖它，不阻塞 | 有新用例再测 |
| OPEN-05 | PCB 丝印版本登记 | 不阻塞 | 下次现场登记 |
| OPEN-06 | 机器人具体硬件与安全机制 | 当前不属于开发范围 | 采购/确定型号后重新开领域设计 |
| OPEN-07 | ROS 2 与目标板 OS 兼容性 | 当前不阻塞 | 未来真要部署机器人时重评，不提前固定发行版 |
| OPEN-08 | tract 模型覆盖与性能 | 不阻塞 ORT 基线与 Inference API | 作为可选 CPU Backend 候选评测 |
| OPEN-09 | PREEMPT_RT 重编及厂商模块兼容 | 不阻塞当前非硬实时系统服务 | 有实时闭环需求后评估，需重出相关 `.ko` |
| OPEN-10 | SMMU DT 布线与真正设备隔离 | 当前用户态安全模型需明确限制；是否改造待论证 | 做威胁模型、IOMMU group 和 DMA 隔离验证 |
| OPEN-11 | 相机人脸能否接受钉单核 | 不阻塞 | 需与服务方共同测，见 `11-cpu-resource-allocation.md` §11.7 |
| OPEN-12 | 核分配是否需要跨重启持久（systemd drop-in vs 运行时 API） | 不阻塞 | 见 `11-cpu-resource-allocation.md` §11.7 |

---

## 相关

- `../../architecture/10-decisions-and-open-questions.md` —— 通用 ADR 与决策模板
- `07-platform-baseline.md` §7.7 —— 已闭合与剩余事项（平台能力视角）
- `11-cpu-resource-allocation.md` §11.7 —— 核分配相关未决

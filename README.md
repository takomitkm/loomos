# loomos

HA4FPAI 的 **riir（Rust 重写 / 重构）** 目标仓库。

HA4FPAI 已转入重构期，其默认分支 `riir` 仅作「指针 + 分支地图」用途（见 [HA4FPAI@riir](https://github.com/WCIS-Team/HA4FPAI/tree/riir)）；实际代码在本仓库演进。

## 迁出说明

2026-11-11 起，本仓库由组织 [WCIS-Team](https://github.com/WCIS-Team) 迁出，归属个人账号 [takomitkm](https://github.com/takomitkm)：由于太菜了没能晋级fpgachina且该项目完全由我个人创建，遂迁出WCIS-Team。旧地址 <https://github.com/WCIS-Team/loomos> 由 GitHub 自动重定向到本仓库，提交历史完整保留。

todo

- [x] agent四件套（[AGENTS.md](AGENTS.md) / [agent/tasks/](agent/) / [agent/run-prototype.sh](agent/run-prototype.sh) / reports 验收正本）
- [x] 用户文档规则（[docs/user/README.md](docs/user/README.md)）
- [ ] context7 4 Rust
- [x] GitHub Actions（[ci.yml](.github/workflows/ci.yml)：Host/Cross/fmt）+ Environments + Deployments + 自定义部署器（后三项仍待做）。

## 上游分支地图（HA4FPAI）

| 分支 / 仓库 | 含义 |
|---|---|
| [`archive-2026-11-06`](https://github.com/WCIS-Team/HA4FPAI/tree/archive-2026-11-06) | 第一版 |
| [`ha-small`](https://github.com/WCIS-Team/HA4FPAI/tree/ha-small) | HA 裁剪版 |
| [`rebuild-uncompleted-2026-11-09`](https://github.com/WCIS-Team/HA4FPAI/tree/rebuild-uncompleted-2026-11-09) | 第一版构架优化（未完成音频链路（怕吵到舍友），外接视频链路） |
| [takomitkm/loomos](https://github.com/takomitkm/loomos)（本仓库） | riir 重构目标仓库 |


---

# normos 原型（2026-11-19 并入 main）

HA4FPAI 各分支用途的 Rust 重做原型：**一键可执行**（agent 或人跑通全链并产出证据报告）。
旧功能按用途保留、实现大量删减，取舍清单见 [docs/dev/PROTOTYPE.md](docs/dev/PROTOTYPE.md)。

## 30 秒上手

```bash
# 一键原型验收（全绿退出码 0）
bash agent/run-prototype.sh
```

它做五件事：环境自检 → `cargo test --workspace`（135 项测试）→ 生成 12 条中文语音夹具
→ 装配整台 normos 并回放全场景（唤醒门控/人脸识别（自研视频路流水线）/四道闸开门/灯控/录制/状态/播报落盘）
→ 写出证据报告 `reports/prototype-report.md`。

启动控制台（旧 10900 统一控制台的新架构版）：

```bash
cargo run -p normos-cli -- serve
curl -X POST http://127.0.0.1:10900/handle_text -d '{"text":"小克，打开客厅的灯"}'
```

## 原型仓库结构

| 路径 | 内容 |
|---|---|
| `crates/normos-runtime` | ID / 资源租约（可观测确认点）/ Capability（主体聚合+generation 撤销）/ 配置服务（Schema·Revision·Outbox·Desired-Applied）/ 组件生命周期 / Supervisor（四类故障×三级恢复）/ 事件日志 |
| `crates/normos-inference` | 推理编排契约：排队/准入/取消语义、Buffer 所有权 **typestate**（未确认完成不得释放）、单 Owner、隔离区 |
| `crates/normos-device` | 设备核心（描述符/提供者/Operation 状态机/租约/遥测/故障）+ mock 家居提供者 + 板端 HTTP 适配器（8091/8092/8093 契约） |
| `crates/normos-audio` | WAV、重采样（linear 升采样 / decimate3-mavg3 降采样——沿旧板实测选型）、能量门/VAD、KWS/ASR/TTS 档位（默认档 echo/sine 全自检） |
| `crates/normos-nlu` | 实体/区域注册表、中文规则语法、直连整句表（不做子串）、klar v2 外部引擎适配契约 |
| `crates/normos-vision` | **视频路（自研可拆卸，契约对齐 fmql30tai-sdi-npu-face-recognition）**：帧源→检测→嵌入→匹配→PAD 六态→录制→直播；引擎全 trait 化，装配模式 Detached/MockDevices/Pipeline/HttpBoard |
| `crates/normos-home` | 意图→设备操作映射、**四道闸开门**（口令→人脸→PAD→声纹）、声纹 mock、危险动作一次性确认（TTL/绑定/用过即废） |
| `crates/normos-executor` | 双通道执行器 + 固定应答 + TTS 文本映射 |
| `crates/normosd` | 装配（Manifest/能力授权/生命周期/supervisor/VisionMode）+ 语音管线 + 控制台 HTTP |
| `crates/normos-cli` | `gen-fixtures` / **`smoke`（一键验收）** / `replay` / `say` / `serve` |
| `docs/dev/engineering/` | 工程纲要包（01-总体设计纲要 / 02-Agent执行规范 / 安全契约模板） |
| `agent/` | Agent 执行规范（[AGENTS.md](AGENTS.md)）、任务提示词、一键脚本、证据产出 |
| `manifests/` | 组件 Manifest（TOML）：voice-pipeline / vision |
| `fixtures/` · `reports/` | 语音夹具 / 验收报告（由脚本生成，不得手改） |

## 原型验证矩阵（architecture/08 §8.5 三层）

| 层 | 内容 | 入口 |
|---|---|---|
| Host CI | fmt / clippy -D warnings / 135 项测试 / smoke 12/12 | `bash agent/run-prototype.sh` |
| Cross CI | aarch64 `cargo check`（无需 linker） | `.github/workflows/ci.yml` |
| Hardware CI | 板上 smoke / RSS / 延迟（风险等级 L0–L3） | 上板时另行记录，**不以宿主数值替代** |

## 原型定位声明

研究原型：真实 sherpa-onnx / 厂商 NPU Runtime / klar 引擎 / 物理设备都走同一 trait 的
适配器档位，默认档全 mock 全自检（旧 lang/config 的 echo/sine 纪律）。所有证据只到
`Tested` / `TypeChecked` 级；不宣称形式化证明。板上实测数据引自旧仓库并标注来源
（[docs/dev/EVIDENCE.md](docs/dev/EVIDENCE.md)）。
